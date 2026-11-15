#!/usr/bin/env bash
#
# Rebuild the gem-store fixtures (#81) from `src/`, byte for byte:
#
#   crates/spinel-cli/tests/fixtures/gem_store/build.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/gem_store/gems
#
# Each `src/<gem>/` is built by `gem build`, which is what makes them real
# `.gem` files rather than tars put together by hand. `SOURCE_DATE_EPOCH` fixes
# every timestamp `gem build` writes (tar headers, gzip headers, the spec's
# date), and each source is copied with fixed modes first, so neither the
# checkout's umask nor the clock changes a byte. The malicious and corrupt
# variants the test needs are made from these by
# `crates/spinel-cli/tests/gem_store.rs` itself, since `gem build` will not
# write them.
#
# Written with RubyGems 4.0.20; another RubyGems may lay the files out
# differently, which `git diff` shows.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-gem-store.XXXXXX")"
trap 'rm -rf "$work"' EXIT

export SOURCE_DATE_EPOCH=1700000000
mkdir -p "$here/gems"
for src in "$here"/src/*/; do
  name="$(basename "$src")"
  cp -R "$src" "$work/$name"
  find "$work/$name" -type d -exec chmod 0755 {} +
  find "$work/$name" -type f -exec chmod 0644 {} +
  find "$work/$name" -type f -path '*/exe/*' -exec chmod 0755 {} +
  (cd "$work/$name" && gem build --silent "$name.gemspec")
  mv "$work/$name"/*.gem "$here/gems/"
done
