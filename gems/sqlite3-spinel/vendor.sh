#!/usr/bin/env bash
#
# Vendor upstream sqlite3's Ruby half into this gem:
#
#   gems/sqlite3-spinel/vendor.sh           re-vendor, overwriting upstream's files
#   gems/sqlite3-spinel/vendor.sh --check   fail if lib/ differs from upstream
#
# `lib/sqlite3.rb` and `lib/sqlite3/*.rb` are the sqlite3 gem's own, verbatim,
# from the packaged gem at the release this gem stands in for (its `ruby`
# build, checked against the SHA-256 rubygems.org lists), and
# `LICENSE-sqlite3-ruby` is its LICENSE. The one file that is not upstream's
# is `lib/sqlite3/sqlite3_native.rb`, Spinel's C half, which upstream's
# `lib/sqlite3.rb` requires where CRuby loads `sqlite3_native.so`; nothing
# of upstream's is patched.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
version="2.10.2"
sha256="f1ea68cd3287f8a49f39a088ac9e4bd0ef4e7f394c3d4a60dd4c7e44b0566964"
ours="lib/sqlite3/sqlite3_native.rb"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -sSfL -o "$tmp/sqlite3.gem" "https://rubygems.org/gems/sqlite3-$version.gem"
if [[ "$( (sha256sum "$tmp/sqlite3.gem" 2>/dev/null || shasum -a 256 "$tmp/sqlite3.gem") | cut -d' ' -f1)" != "$sha256" ]]; then
  echo "vendor.sh: sqlite3-$version.gem is not the gem its checksum names" >&2
  exit 1
fi
mkdir -p "$tmp/gem" "$tmp/data"
tar -xf "$tmp/sqlite3.gem" -C "$tmp/gem"
tar -xzf "$tmp/gem/data.tar.gz" -C "$tmp/data" lib LICENSE

if [[ "${1:-}" == "--check" ]]; then
  status=0
  upstream="$(cd "$tmp/data" && find lib -name '*.rb' | sort)"
  vendored="$(cd "$here" && find lib -name '*.rb' ! -path "$ours" | sort)"
  if [[ "$upstream" != "$vendored" ]]; then
    echo "vendor.sh: lib/ has other files than upstream sqlite3 $version's:" >&2
    diff <(echo "$upstream") <(echo "$vendored") >&2 || true
    status=1
  fi
  while read -r file; do
    if ! cmp -s "$tmp/data/$file" "$here/$file"; then
      echo "vendor.sh: $file differs from upstream sqlite3 $version's" >&2
      status=1
    fi
  done <<<"$upstream"
  if ! cmp -s "$tmp/data/LICENSE" "$here/LICENSE-sqlite3-ruby"; then
    echo "vendor.sh: LICENSE-sqlite3-ruby differs from upstream sqlite3 $version's LICENSE" >&2
    status=1
  fi
  [[ $status -eq 0 ]] && echo "gems/sqlite3-spinel/lib is upstream sqlite3 $version's"
  exit $status
fi

cp -R "$tmp/data/lib/." "$here/lib/"
cp "$tmp/data/LICENSE" "$here/LICENSE-sqlite3-ruby"
echo "vendored upstream sqlite3 $version's lib/ into gems/sqlite3-spinel"
