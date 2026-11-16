#!/usr/bin/env bash
#
# Take the resolver's compact index snapshot (#78) from rubygems.org:
#
#   crates/spinel-cli/tests/fixtures/resolver/snapshot.sh
#   crates/spinel-cli/tests/fixtures/resolver/bundler.sh
#
# `index/info/<gem>` is rubygems.org's `/info/<gem>` for every gem the roots
# below reach through any version's dependencies, so every version the
# resolver may consider is there with its real dependencies, `ruby:` and
# `rubygems:` requirements, platforms and checksums. One trim: the
# `created_at:` requirement is dropped from each line, since only Bundler's
# cooldown reads it and it is a third of the bytes. `index/versions` is made
# from those files, as rubygems.org lays `/versions` out, each gem's line
# ending in the MD5 of its info file, so the index client fetches each one
# once. `index/SOURCE` records when the snapshot was taken.
#
# Taking it again moves every gem to what rubygems.org has now, so run
# `bundler.sh` after it: that rewrites the expected resolutions, which the
# test compares Spinel's against.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
roots=(sinatra rackup webrick json)

rm -rf "$here/index"
mkdir -p "$here/index/info"
seen=" "
todo=("${roots[@]}")
while ((${#todo[@]})); do
  gem="${todo[0]}"
  todo=("${todo[@]:1}")
  [[ "$seen" == *" $gem "* ]] && continue
  seen+="$gem "
  curl -sSf "https://rubygems.org/info/$gem" |
    sed -E 's/,created_at:[^,|]*//; s/\|created_at:[^,]*,?/|/' >"$here/index/info/$gem"
  # The dependencies are what comes before `|`, after the version.
  while read -r dep; do
    [[ -n "$dep" ]] && todo+=("$dep")
  done < <(grep -v '^---$' "$here/index/info/$gem" | cut -d'|' -f1 |
    awk 'index($0, " ") { print substr($0, index($0, " ") + 1) }' |
    tr ',' '\n' | cut -d: -f1 | sort -u)
done

{
  echo "created_at: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "---"
  for info in "$here"/index/info/*; do
    gem="$(basename "$info")"
    versions="$(grep -v '^---$' "$info" | cut -d' ' -f1 | paste -sd, -)"
    echo "$gem $versions $(md5sum "$info" | cut -d' ' -f1)"
  done
} >"$here/index/versions"

echo "rubygems.org compact index, /info of ${roots[*]} and what they reach, taken $(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$here/index/SOURCE"
