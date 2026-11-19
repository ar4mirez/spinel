#!/usr/bin/env bash
#
# Vendor upstream nio4r into this gem and its test:
#
#   gems/nio4r-spinel/vendor.sh           re-vendor, overwriting upstream's files
#   gems/nio4r-spinel/vendor.sh --check   fail if anything vendored differs from upstream
#
# `lib/nio.rb`, `lib/nio4r.rb` and `lib/nio/*.rb` are the nio4r gem's own,
# verbatim, from the packaged gem at the release this gem stands in for (its
# `ruby` build, checked against the SHA-256 rubygems.org lists), and
# `LICENSE-nio4r` is its `license.md`. The one file in `lib/` that is not
# upstream's is `lib/nio4r_ext.rb`, Spinel's C half, which upstream's
# `lib/nio.rb` requires where CRuby loads `nio4r_ext.so`; nothing of
# upstream's is patched.
#
# nio4r's test suite is not in its gem, so `spec/` comes from the source
# archive GitHub serves for the release's tag, checked against the SHA-256
# below, into `crates/spinel-cli/tests/fixtures/nio4r/app/spec/`, verbatim,
# where `tests/nio4r.rs` runs it.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
specs="$here/../../crates/spinel-cli/tests/fixtures/nio4r/app/spec"
version="2.7.5"
gem_sha256="6c90168e48fb5f8e768419c93abb94ba2b892a1d0602cb06eef16d8b7df1dca1"
source_sha256="6b17580f7a84903b5c76a97bb926098cfeb012fd6189a0481f417df6d944d93f"
ours="lib/nio4r_ext.rb"

sha256() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -sSfL -o "$tmp/nio4r.gem" "https://rubygems.org/gems/nio4r-$version.gem"
if [[ "$(sha256 "$tmp/nio4r.gem")" != "$gem_sha256" ]]; then
  echo "vendor.sh: nio4r-$version.gem is not the gem its checksum names" >&2
  exit 1
fi
curl -sSfL -o "$tmp/source.tar.gz" "https://github.com/socketry/nio4r/archive/refs/tags/v$version.tar.gz"
if [[ "$(sha256 "$tmp/source.tar.gz")" != "$source_sha256" ]]; then
  echo "vendor.sh: nio4r v$version's source archive is not the one its checksum names" >&2
  exit 1
fi
mkdir -p "$tmp/gem" "$tmp/data" "$tmp/source"
tar -xf "$tmp/nio4r.gem" -C "$tmp/gem"
tar -xzf "$tmp/gem/data.tar.gz" -C "$tmp/data" lib license.md
tar -xzf "$tmp/source.tar.gz" -C "$tmp/source" --strip-components=1 "nio4r-$version/spec"

if [[ "${1:-}" == "--check" ]]; then
  status=0
  compare() { # what, upstream dir, vendored dir, files to leave out of the vendored side
    local what="$1" upstream="$2" vendored="$3" skip="${4:-}"
    local theirs mine
    theirs="$(cd "$upstream" && find . -type f | sort)"
    mine="$(cd "$vendored" && find . -type f | grep -vxF "./$skip" | sort)"
    if [[ "$theirs" != "$mine" ]]; then
      echo "vendor.sh: $what has other files than upstream nio4r $version's:" >&2
      diff <(echo "$theirs") <(echo "$mine") >&2 || true
      status=1
    fi
    while read -r file; do
      if ! cmp -s "$upstream/$file" "$vendored/$file"; then
        echo "vendor.sh: $what/${file#./} differs from upstream nio4r $version's" >&2
        status=1
      fi
    done <<<"$theirs"
  }
  compare "gems/nio4r-spinel/lib" "$tmp/data/lib" "$here/lib" "${ours#lib/}"
  compare "crates/spinel-cli/tests/fixtures/nio4r/app/spec" "$tmp/source/spec" "$specs"
  if ! cmp -s "$tmp/data/license.md" "$here/LICENSE-nio4r"; then
    echo "vendor.sh: LICENSE-nio4r differs from upstream nio4r $version's license.md" >&2
    status=1
  fi
  [[ $status -eq 0 ]] && echo "gems/nio4r-spinel/lib and the nio4r fixture's spec/ are upstream nio4r $version's"
  exit $status
fi

cp -R "$tmp/data/lib/." "$here/lib/"
cp "$tmp/data/license.md" "$here/LICENSE-nio4r"
mkdir -p "$specs"
cp -R "$tmp/source/spec/." "$specs/"
echo "vendored upstream nio4r $version's lib/ into gems/nio4r-spinel and its spec/ into the nio4r fixture"
