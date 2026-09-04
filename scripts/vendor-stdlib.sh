#!/usr/bin/env bash
#
# Vendor Ruby's pure-Ruby standard library into `stdlib/`.
#
#   scripts/vendor-stdlib.sh           re-vendor, overwriting `stdlib/`
#   scripts/vendor-stdlib.sh --check   fail if `stdlib/` differs from upstream
#
# `stdlib/` is upstream `lib/` flattened to the root, so `stdlib/` is itself a
# `$LOAD_PATH` entry: `stdlib/set.rb`, `stdlib/net/http.rb`. Two directories are
# ours and not upstream's, and are skipped by the diff: `UPSTREAM` records the
# pin, `LICENSE/` holds Ruby's license files.
#
# Not a `git subtree`: `git subtree add` copies a whole repository, and we want
# one directory out of ruby/ruby. `git subtree split -P lib` could synthesise
# one, but only from a full clone, and the result shares no history with
# upstream, so `git subtree pull` would never merge cleanly. A pinned copy plus
# the drift check below buys everything a subtree would have, for less.
#
# To bump: edit RUBY_TAG, run this script, commit `stdlib/` in its own commit.

set -euo pipefail

# The language version Spinel targets. README.md: "one Ruby language version at
# a time, the latest stable".
RUBY_TAG="v4.0.6"
RUBY_REPO="https://github.com/ruby/ruby.git"

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
cat > "$staged/UPSTREAM" <<EOF
Ruby's pure-Ruby standard library, vendored verbatim.

    upstream  $RUBY_REPO
    tag       $RUBY_TAG
    commit    $sha
    path      lib/ (flattened to this directory, so stdlib/ is a \$LOAD_PATH root)

Licenses are upstream's, in LICENSE/. LEGAL records the per-file terms for the
files here that are not under Ruby's own license.

Do not edit anything under stdlib/ by hand. CI diffs this tree against the tag
above and fails on any difference. To update, edit RUBY_TAG in
scripts/vendor-stdlib.sh and run it.
EOF

if $check_only; then
  # `stdlib/` must match the staged tree exactly. Every difference is a failure,
  # which is stricter than "unexplained drift" and needs no allowlist to keep
  # honest. When a patch to a vendored file is genuinely needed, this is the
  # place to add one, with the reason next to it.
  if diff -r -q "$staged" "$stdlib"; then
    files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
    echo "stdlib/ matches ruby/ruby $RUBY_TAG ($files .rb files)"
  else
    echo "::error::stdlib/ has drifted from ruby/ruby $RUBY_TAG ($sha)" >&2
    echo "run scripts/vendor-stdlib.sh and commit the result" >&2
    exit 1
  fi
else
  rm -rf "$stdlib"
  mv "$staged" "$stdlib"
  files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
  echo "vendored ruby/ruby $RUBY_TAG into stdlib/ ($files .rb files)"
fi
