#!/usr/bin/env bash
#
# Take what `crates/spinel-cli/tests/install.rs` serves on 127.0.0.1 in
# place of rubygems.org (#84), from rubygems.org:
#
#   crates/spinel-cli/tests/fixtures/install/snapshot.sh
#
# `index/info/rake` is rubygems.org's `/info/rake`, every version, with the
# `created_at:` requirement dropped as the resolver's snapshot drops it
# (`../resolver/snapshot.sh`), and `index/versions` lists it as rubygems.org
# lays `/versions` out, its line ending in the MD5 of that file. That is the
# index `../plain/Gemfile` resolves against. `gems/` holds the real `.gem`
# files: the newest rake, which a fresh resolution of `plain` chooses, and
# every gem `../sinatra-lock/Gemfile.lock` locks. Each is checked against the
# SHA-256 its `/info` line or the lockfile's `CHECKSUMS` lists before it is
# kept, which is what `spinel install` checks it against too.
#
# Taking it again moves rake to what rubygems.org has now; the test reads
# the version from `index/info/rake`, so only the files change.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
lock="$here/../sinatra-lock/Gemfile.lock"

rm -rf "$here/index" "$here/gems"
mkdir -p "$here/index/info" "$here/gems"
curl -sSf "https://rubygems.org/info/rake" |
  sed -E 's/,created_at:[^,|]*//; s/\|created_at:[^,]*,?/|/' >"$here/index/info/rake"
{
  echo "created_at: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "---"
  versions="$(grep -v '^---$' "$here/index/info/rake" | cut -d' ' -f1 | paste -sd, -)"
  echo "rake $versions $(md5sum "$here/index/info/rake" | cut -d' ' -f1)"
} >"$here/index/versions"

# `name version sha256`, one line per gem to keep.
wanted="$(
  # The newest rake that is not a prerelease, as a fresh resolution takes it.
  grep -v '^---$' "$here/index/info/rake" | grep -v '^[^ ]*[a-zA-Z]' | tail -1 |
    sed -E 's/^([^ ]+) .*checksum:([0-9a-f]+).*/rake \1 \2/'
  # The Sinatra lockfile's `CHECKSUMS`: `  name (version) sha256=hex`.
  sed -n '/^CHECKSUMS$/,/^$/p' "$lock" |
    sed -nE 's/^  ([^ ]+) \(([^)]+)\) sha256=([0-9a-f]+)$/\1 \2 \3/p'
)"
while read -r name version sha; do
  file="$here/gems/$name-$version.gem"
  curl -sSfL -o "$file" "https://rubygems.org/gems/$name-$version.gem"
  if [[ "$(sha256sum "$file" | cut -d' ' -f1)" != "$sha" ]]; then
    echo "snapshot.sh: $name-$version.gem is not the gem its checksum names" >&2
    exit 1
  fi
done <<<"$wanted"

echo "rubygems.org: /info/rake and the .gem files above, taken $(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$here/SOURCE"
