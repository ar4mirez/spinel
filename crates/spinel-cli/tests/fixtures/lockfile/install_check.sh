#!/usr/bin/env bash
#
# Does `bundle install` accept the lockfile Spinel writes, and leave it as it
# is (#80)? An opt-in check, never run by CI, since it needs Ruby and Bundler:
#
#   cargo build --release
#   crates/spinel-cli/tests/fixtures/lockfile/install_check.sh [case...]
#
# For each case under `cases/` Spinel can resolve (by default `rails`,
# `multi_platform` and `ruby_version`), `spinel resolve --lockfile` writes
# its `Gemfile.lock` against the snapshot in `index/`. Installing needs the
# gems themselves, and nothing here reaches rubygems.org, so
# `dummy_gems.rb` builds a stand-in for every spec that lockfile locks and
# puts their checksums in a copy of the index; Spinel writes the lockfile
# again against that copy (it may differ from the first only in those
# checksums), and Bundler, run as itself on this machine's Ruby, installs it
# from the copy into a scratch `BUNDLE_PATH` with `frozen` set, which fails if
# the lockfile would change, then once more without it. The lockfile must
# come out of both byte for byte as Spinel wrote it.
#
# SPINEL names the binary (default `target/release/spinel`); KEEP_WORK=1
# keeps the scratch directory, under TMPDIR, to look at. Written with
# Bundler 4.0.20 on Ruby 4.0.9 (x86_64-linux); Python 3 serves the files.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../../.." && pwd)"
spinel="${SPINEL:-$root/target/release/spinel}"
cases=("$@")
((${#cases[@]})) || cases=(rails multi_platform ruby_version)
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-install-check.XXXXXX")"
server=""
cleanup() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null
  [[ -n "${KEEP_WORK:-}" ]] || rm -rf "$work"
}
trap cleanup EXIT

# Serve `$1` and set `url`, stopping the server before it.
serve() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null
  python3 -u -m http.server --bind 127.0.0.1 0 --directory "$1" >"$work/server.log" 2>&1 &
  server=$!
  local port=""
  for _ in $(seq 50); do
    port="$(sed -n 's/.* port \([0-9]*\) .*/\1/p' "$work/server.log" | head -1)"
    [[ -n "$port" ]] && break
    sleep 0.1
  done
  [[ -n "$port" ]] || { cat "$work/server.log" >&2; exit 1; }
  url="http://127.0.0.1:$port"
}

# `spinel resolve --lockfile` for case `$1`, against `$url`, into `$2`.
spinel_lock() {
  local dir="$here/cases/$1" args=()
  for platform in $(sed -n 's/^platforms=//p' "$dir/case.env"); do
    args+=(--platform "$platform")
  done
  (cd "$dir" && SPINEL_RESOLVER_SOURCE="$url" "$spinel" run "$root/shims/gemfile_eval.rb") |
    SPINEL_HOME="$work/spinel-home-$1" "$spinel" resolve --lockfile "${args[@]}" \
      --ruby-version "$(sed -n 's/^ruby=//p' "$dir/case.env")" \
      --rubygems-version "$(sed -n 's/^rubygems=//p' "$dir/case.env")" - >"$2"
  rm -rf "$work/spinel-home-$1"
}

status=0
for name in "${cases[@]}"; do
  case_work="$work/$name"
  mkdir -p "$case_work/app"
  cp "$here/cases/$name/Gemfile" "$case_work/app/Gemfile"

  serve "$here/index"
  spinel_lock "$name" "$case_work/first.lock"

  mkdir -p "$case_work/source"
  cp -R "$here/index/." "$case_work/source/"
  ruby "$here/dummy_gems.rb" "$case_work/first.lock" "$case_work/source" "$case_work/source/gems" |
    sed "s/^/$name: /"

  serve "$case_work/source"
  spinel_lock "$name" "$case_work/app/Gemfile.lock"
  cp "$case_work/app/Gemfile.lock" "$case_work/spinel.lock"
  # The two differ in the server's port too.
  if ! diff <(sed -e 's/ sha256=[0-9a-f]*//' -e '/^  remote: /d' "$case_work/first.lock") \
    <(sed -e 's/ sha256=[0-9a-f]*//' -e '/^  remote: /d' "$case_work/spinel.lock") >/dev/null; then
    echo "$name: the stand-ins' checksums changed more than the checksums" >&2
    status=1
    continue
  fi

  for frozen in true false; do
    if ! (
      cd "$case_work/app"
      env -i PATH="$PATH" HOME="$case_work/home" \
        BUNDLE_GEMFILE="$case_work/app/Gemfile" \
        BUNDLE_APP_CONFIG="$case_work/app/.bundle" \
        BUNDLE_USER_HOME="$case_work/bundle-home" \
        BUNDLE_PATH="$case_work/vendor" \
        BUNDLE_FROZEN="$frozen" \
        SPINEL_RESOLVER_SOURCE="$url" \
        bundle install >"$case_work/install-$frozen.log" 2>&1
    ); then
      echo "$name: bundle install (frozen=$frozen) failed:" >&2
      cat "$case_work/install-$frozen.log" >&2
      status=1
      continue 2
    fi
    if ! cmp -s "$case_work/spinel.lock" "$case_work/app/Gemfile.lock"; then
      echo "$name: bundle install (frozen=$frozen) changed the lockfile:" >&2
      diff "$case_work/spinel.lock" "$case_work/app/Gemfile.lock" >&2 || true
      status=1
      continue 2
    fi
  done
  gems="$(grep -c '^Installing\|^Using' "$case_work/install-true.log" || true)"
  echo "$name: bundle install accepted Spinel's lockfile unchanged ($gems gems installed)"
done
exit "$status"
