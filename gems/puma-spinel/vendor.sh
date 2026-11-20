#!/usr/bin/env bash
#
# Vendor upstream Puma into this gem and its test:
#
#   gems/puma-spinel/vendor.sh           re-vendor, overwriting upstream's files
#   gems/puma-spinel/vendor.sh --check   fail if anything vendored differs from upstream
#
# `lib/` and `bin/` are the puma gem's own, verbatim, from the packaged gem at
# the release this gem stands in for (its `ruby` build, checked against the
# SHA-256 rubygems.org lists), and `LICENSE-puma` is its `LICENSE`. The one
# file in `lib/` that is not upstream's is `lib/puma/puma_http11.rb`,
# Spinel's C half, which upstream's `lib/puma.rb` and `lib/puma/minissl.rb`
# require where CRuby loads `puma_http11.so`; nothing of upstream's is
# patched.
#
# `ext/puma_spinel/ragel/` holds the three files of upstream's
# `ext/puma_http11/` the HTTP parser is made from: the grammar
# (`http11_parser.rl`, `http11_parser_common.rl`) and the C Ragel wrote from
# it (`http11_parser.c`), which `ext/puma_spinel/tables.sh` checks its
# tables against.
#
# Puma's test suite is not in its gem, so `test/` comes from the source
# archive GitHub serves for the release's tag, checked against the SHA-256
# below, into `crates/spinel-cli/tests/fixtures/puma/app/test/`, verbatim,
# where `tests/puma.rs` runs it, with the archive's `puma.gemspec` beside it:
# the suite's Bundler fixtures name Puma by its path, `gem "puma", path:`.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app="$here/../../crates/spinel-cli/tests/fixtures/puma/app"
tests="$app/test"
ragel="$here/ext/puma_spinel/ragel"
version="8.1.2"
gem_sha256="3a6d9a34c9a4d0175eabd11b703939b5d43ebe08bc2534ac7dfd4df33a6ceedc"
source_sha256="ed3c9667092b5cefea2615342c355786feaac71805858d82e4dfb9f1ca5dad8a"
ours="lib/puma/puma_http11.rb"
grammar=(http11_parser.rl http11_parser_common.rl http11_parser.c)

sha256() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -sSfL -o "$tmp/puma.gem" "https://rubygems.org/gems/puma-$version.gem"
if [[ "$(sha256 "$tmp/puma.gem")" != "$gem_sha256" ]]; then
  echo "vendor.sh: puma-$version.gem is not the gem its checksum names" >&2
  exit 1
fi
curl -sSfL -o "$tmp/source.tar.gz" "https://github.com/puma/puma/archive/refs/tags/v$version.tar.gz"
if [[ "$(sha256 "$tmp/source.tar.gz")" != "$source_sha256" ]]; then
  echo "vendor.sh: puma v$version's source archive is not the one its checksum names" >&2
  exit 1
fi
mkdir -p "$tmp/gem" "$tmp/data" "$tmp/source"
tar -xf "$tmp/puma.gem" -C "$tmp/gem"
tar -xzf "$tmp/gem/data.tar.gz" -C "$tmp/data" lib bin LICENSE
tar -xzf "$tmp/source.tar.gz" -C "$tmp/source" --strip-components=1 "puma-$version/test" \
  "puma-$version/puma.gemspec" \
  "${grammar[@]/#/puma-$version/ext/puma_http11/}"

if [[ "${1:-}" == "--check" ]]; then
  status=0
  compare() { # what, upstream dir, vendored dir, file to leave out of the vendored side
    local what="$1" upstream="$2" vendored="$3" skip="${4:-}"
    local theirs mine
    theirs="$(cd "$upstream" && find . -type f | sort)"
    mine="$(cd "$vendored" && find . -type f | grep -vxF "./$skip" | sort)"
    if [[ "$theirs" != "$mine" ]]; then
      echo "vendor.sh: $what has other files than upstream puma $version's:" >&2
      diff <(echo "$theirs") <(echo "$mine") >&2 || true
      status=1
    fi
    while read -r file; do
      if ! cmp -s "$upstream/$file" "$vendored/$file"; then
        echo "vendor.sh: $what/${file#./} differs from upstream puma $version's" >&2
        status=1
      fi
    done <<<"$theirs"
  }
  compare "gems/puma-spinel/lib" "$tmp/data/lib" "$here/lib" "${ours#lib/}"
  compare "gems/puma-spinel/bin" "$tmp/data/bin" "$here/bin"
  compare "gems/puma-spinel/ext/puma_spinel/ragel" "$tmp/source/ext/puma_http11" "$ragel"
  compare "crates/spinel-cli/tests/fixtures/puma/app/test" "$tmp/source/test" "$tests"
  if ! cmp -s "$tmp/source/puma.gemspec" "$app/puma.gemspec"; then
    echo "vendor.sh: the puma fixture's puma.gemspec differs from upstream puma $version's" >&2
    status=1
  fi
  if ! cmp -s "$tmp/data/LICENSE" "$here/LICENSE-puma"; then
    echo "vendor.sh: LICENSE-puma differs from upstream puma $version's LICENSE" >&2
    status=1
  fi
  [[ $status -eq 0 ]] && echo "gems/puma-spinel's lib/, bin/ and ragel/, and the puma fixture's test/ and puma.gemspec, are upstream puma $version's"
  exit $status
fi

cp -R "$tmp/data/lib/." "$here/lib/"
mkdir -p "$here/bin" "$ragel" "$tests"
cp -R "$tmp/data/bin/." "$here/bin/"
cp "$tmp/data/LICENSE" "$here/LICENSE-puma"
cp -R "$tmp/source/ext/puma_http11/." "$ragel/"
cp -R "$tmp/source/test/." "$tests/"
cp "$tmp/source/puma.gemspec" "$app/puma.gemspec"
echo "vendored upstream puma $version's lib/ and bin/ into gems/puma-spinel, its parser grammar into ext/puma_spinel/ragel, and its test/ and puma.gemspec into the puma fixture"
