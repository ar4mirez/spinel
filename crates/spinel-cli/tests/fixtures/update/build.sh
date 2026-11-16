#!/usr/bin/env bash
#
# Rebuild the `spinel update` fixtures (#87) from `src/`, byte for byte:
#
#   crates/spinel-cli/tests/fixtures/update/build.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/update/gems
#
# Each `src/<gem>-<version>/` is built by `gem build`, so they are real `.gem`
# files. `SOURCE_DATE_EPOCH` fixes every timestamp `gem build` writes, and
# each source is copied with fixed modes first, so neither the checkout's
# umask nor the clock changes a byte. `crates/spinel-cli/tests/update.rs` serves
# them with a compact index it writes from their checksums.
#
# Written with RubyGems 4.0.20; another RubyGems may lay the files out
# differently, which `git diff` shows.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-update.XXXXXX")"
trap 'rm -rf "$work"' EXIT

export SOURCE_DATE_EPOCH=1700000000
mkdir -p "$here/gems"
for src in "$here"/src/*/; do
  dir="$(basename "$src")"
  name="${dir%-*}"
  cp -R "$src" "$work/$dir"
  find "$work/$dir" -type d -exec chmod 0755 {} +
  find "$work/$dir" -type f -exec chmod 0644 {} +
  (cd "$work/$dir" && gem build --silent "$name.gemspec")
  mv "$work/$dir"/*.gem "$here/gems/"
done
