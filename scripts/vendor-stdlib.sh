#!/usr/bin/env bash
#
# Vendor Ruby's pure-Ruby standard library into `stdlib/`.
#
#   scripts/vendor-stdlib.sh           re-vendor, overwriting `stdlib/`
#   scripts/vendor-stdlib.sh --check   fail if `stdlib/` differs from upstream
#
# `stdlib/` is upstream `lib/` flattened to the root, so `stdlib/` is itself a
# `$LOAD_PATH` entry: `stdlib/set.rb`, `stdlib/net/http.rb`. `UPSTREAM` records
# the pins and `LICENSE/` holds the license files; both are written here too,
# so the check covers them.
#
# One gem is laid over it the same way: `webrick` (#54), which left the
# standard library in Ruby 3.0 but is what the phase 4 milestone serves a
# Sinatra app on. Its `lib/` lands beside Ruby's, verbatim, at a pinned
# release, and its license in `LICENSE/webrick/`. No file of one may shadow a
# file of the other; the script stops if one would.
#
# Not a `git subtree`: `git subtree add` copies a whole repository, and we want
# one directory out of ruby/ruby. `git subtree split -P lib` could synthesise
# one, but only from a full clone, and the result shares no history with
# upstream, so `git subtree pull` would never merge cleanly. A pinned copy plus
# the drift check below buys everything a subtree would have, for less.
#
# To bump: edit RUBY_TAG or WEBRICK_TAG, run this script, commit `stdlib/` in
# its own commit.

set -euo pipefail

# The language version Spinel targets. README.md: "one Ruby language version at
# a time, the latest stable".
RUBY_TAG="v4.0.6"
RUBY_REPO="https://github.com/ruby/ruby.git"

# The gem vendored beside it, at a release tag.
WEBRICK_TAG="v1.9.2"
WEBRICK_REPO="https://github.com/ruby/webrick.git"

# Upstream's license files, copied to `stdlib/LICENSE/`. LEGAL is not optional
# paperwork: `lib/` mixes licenses, and LEGAL is the per-file record of which.
LICENSE_FILES=(COPYING COPYING.ja BSDL LEGAL)

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
stdlib="$repo_root/stdlib"

check_only=false
if [[ "${1:-}" == "--check" ]]; then
  check_only=true
elif [[ $# -gt 0 ]]; then
  echo "usage: $(basename "$0") [--check]" >&2
  exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# A blobless, depth-1, sparse fetch: ruby/ruby is large and we want one
# directory of one commit from it.
fetch_upstream() {
  git init --quiet "$work/ruby"
  cd "$work/ruby"
  git remote add origin "$RUBY_REPO"
  git config core.sparseCheckout true
  {
    echo 'lib/'
    printf '/%s\n' "${LICENSE_FILES[@]}"
  } > .git/info/sparse-checkout
  git fetch --quiet --depth 1 --filter=blob:none origin "refs/tags/$RUBY_TAG"
  git checkout --quiet FETCH_HEAD
  cd "$repo_root"
}

echo "fetching ruby/ruby at $RUBY_TAG"
fetch_upstream
sha="$(git -C "$work/ruby" rev-parse FETCH_HEAD)"

# The same fetch for webrick. Its tags are annotated, so the commit is the
# tag's, peeled.
fetch_webrick() {
  git init --quiet "$work/webrick"
  cd "$work/webrick"
  git remote add origin "$WEBRICK_REPO"
  git config core.sparseCheckout true
  printf '%s\n' 'lib/' '/LICENSE.txt' > .git/info/sparse-checkout
  git fetch --quiet --depth 1 --filter=blob:none origin "refs/tags/$WEBRICK_TAG"
  git checkout --quiet FETCH_HEAD
  cd "$repo_root"
}

echo "fetching ruby/webrick at $WEBRICK_TAG"
fetch_webrick
webrick_sha="$(git -C "$work/webrick" rev-parse 'FETCH_HEAD^{commit}')"

# Assemble what `stdlib/` should look like, then either install it or diff
# against it. Building it once and comparing keeps the two modes honest: the
# check tests the same tree the vendor step would write.
staged="$work/staged"
mkdir -p "$staged"
cp -R "$work/ruby/lib/." "$staged/"
mkdir -p "$staged/LICENSE"
for file in "${LICENSE_FILES[@]}"; do
  cp "$work/ruby/$file" "$staged/LICENSE/$file"
done
clashes="$(cd "$work/webrick/lib" && find . -type f | while read -r file; do
  if [[ -e "$staged/$file" ]]; then echo "$file"; fi
done)"
if [[ -n "$clashes" ]]; then
  echo "::error::ruby/webrick $WEBRICK_TAG would replace ruby/ruby files:" >&2
  echo "$clashes" >&2
  exit 1
fi
cp -R "$work/webrick/lib/." "$staged/"
mkdir -p "$staged/LICENSE/webrick"
cp "$work/webrick/LICENSE.txt" "$staged/LICENSE/webrick/LICENSE.txt"
cat > "$staged/UPSTREAM" <<EOF
Ruby's pure-Ruby standard library, vendored verbatim.

    upstream  $RUBY_REPO
    tag       $RUBY_TAG
    commit    $sha
    path      lib/ (flattened to this directory, so stdlib/ is a \$LOAD_PATH root)

Laid over it, also verbatim, the lib/ of one gem that is not in Ruby's
standard library:

    upstream  $WEBRICK_REPO
    tag       $WEBRICK_TAG
    commit    $webrick_sha
    files     webrick.rb, webrick/

Licenses are upstream's, in LICENSE/. LEGAL records the per-file terms for the
files here from ruby/ruby that are not under Ruby's own license; webrick's own
is in LICENSE/webrick/.

Do not edit anything under stdlib/ by hand. CI diffs this tree against the tags
above and fails on any difference. To update, edit RUBY_TAG or WEBRICK_TAG in
scripts/vendor-stdlib.sh and run it.
EOF

if $check_only; then
  # `stdlib/` must match the staged tree exactly. Every difference is a failure,
  # which is stricter than "unexplained drift" and needs no allowlist to keep
  # honest. When a patch to a vendored file is genuinely needed, this is the
  # place to add one, with the reason next to it.
  if diff -r -q "$staged" "$stdlib"; then
    files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
    echo "stdlib/ matches ruby/ruby $RUBY_TAG and ruby/webrick $WEBRICK_TAG ($files .rb files)"
  else
    echo "::error::stdlib/ has drifted from ruby/ruby $RUBY_TAG ($sha) and ruby/webrick $WEBRICK_TAG" >&2
    echo "run scripts/vendor-stdlib.sh and commit the result" >&2
    exit 1
  fi
else
  rm -rf "$stdlib"
  mv "$staged" "$stdlib"
  files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
  echo "vendored ruby/ruby $RUBY_TAG and ruby/webrick $WEBRICK_TAG into stdlib/ ($files .rb files)"
fi
