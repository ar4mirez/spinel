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
# Three gems are laid over it the same way. `webrick` (#54) left the standard
# library in Ruby 3.0 but is what the phase 4 milestone serves a Sinatra app
# on. `json` (#55) is a default gem whose Ruby half lives in ruby/ruby's
# `ext/json/lib`, not `lib/`, so it comes from ruby/json at the release Ruby
# bundles; its C half is Spinel's, in `lib/json/ext/` (docs/engine.md,
# "Built-in gems"). `io-console` (#110) is the same: its Ruby half,
# `io/console/size.rb`, is in ruby/ruby's `ext/io/console/lib`, and its C half
# is Spinel's `lib/io/console.rb`. Each gem's `lib/` lands beside Ruby's, verbatim, at a
# pinned release, and its license in `LICENSE/<gem>/`. No file of one may
# shadow a file of another; the script stops if one would.
#
# Not a `git subtree`: `git subtree add` copies a whole repository, and we want
# one directory out of ruby/ruby. `git subtree split -P lib` could synthesise
# one, but only from a full clone, and the result shares no history with
# upstream, so `git subtree pull` would never merge cleanly. A pinned copy plus
# the drift check below buys everything a subtree would have, for less.
#
# To bump: edit RUBY_TAG, WEBRICK_TAG, JSON_TAG or IO_CONSOLE_TAG, run this
# script, commit `stdlib/` in its own commit.

set -euo pipefail

# The language version Spinel targets. README.md: "one Ruby language version at
# a time, the latest stable".
RUBY_TAG="v4.0.6"
RUBY_REPO="https://github.com/ruby/ruby.git"

# The gems vendored beside it, at release tags.
WEBRICK_TAG="v1.9.2"
WEBRICK_REPO="https://github.com/ruby/webrick.git"
# The json Ruby 4.0 bundles (ext/json/lib/json/version.rb at RUBY_TAG), so the
# built-in is the version Spinel claims to be.
JSON_TAG="v2.18.0"
JSON_REPO="https://github.com/ruby/json.git"
# The io-console Ruby 4.0 bundles (its gemspec in ext/io/console at RUBY_TAG).
IO_CONSOLE_TAG="v0.8.2"
IO_CONSOLE_REPO="https://github.com/ruby/io-console.git"

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

# The same fetch for a gem: its `lib/` and the license files named. Its tags
# are annotated, so the commit is the tag's, peeled.
fetch_gem() {
  local name="$1" repo="$2" tag="$3"
  shift 3
  git init --quiet "$work/$name"
  cd "$work/$name"
  git remote add origin "$repo"
  git config core.sparseCheckout true
  {
    echo 'lib/'
    printf '/%s\n' "$@"
  } > .git/info/sparse-checkout
  git fetch --quiet --depth 1 --filter=blob:none origin "refs/tags/$tag"
  git checkout --quiet FETCH_HEAD
  cd "$repo_root"
}

WEBRICK_LICENSES=(LICENSE.txt)
echo "fetching ruby/webrick at $WEBRICK_TAG"
fetch_gem webrick "$WEBRICK_REPO" "$WEBRICK_TAG" "${WEBRICK_LICENSES[@]}"
webrick_sha="$(git -C "$work/webrick" rev-parse 'FETCH_HEAD^{commit}')"

# json is under Ruby's license or the 2-clause BSDL, as Ruby is. Its LEGAL
# names the C files its extension vendors, none of which land here; it is
# kept because it is the gem's own record of its terms.
JSON_LICENSES=(COPYING BSDL LEGAL)
echo "fetching ruby/json at $JSON_TAG"
fetch_gem json "$JSON_REPO" "$JSON_TAG" "${JSON_LICENSES[@]}"
json_sha="$(git -C "$work/json" rev-parse 'FETCH_HEAD^{commit}')"

# io-console is under Ruby's license or the 2-clause BSDL. Its `lib/ffi/` is
# JRuby's backend, the C half in Ruby over FFI; ruby/ruby does not ship it and
# neither does Spinel, so only `lib/io/` is laid over.
IO_CONSOLE_LICENSES=(COPYING BSDL)
echo "fetching ruby/io-console at $IO_CONSOLE_TAG"
fetch_gem io-console "$IO_CONSOLE_REPO" "$IO_CONSOLE_TAG" "${IO_CONSOLE_LICENSES[@]}"
io_console_sha="$(git -C "$work/io-console" rev-parse 'FETCH_HEAD^{commit}')"
rm -rf "$work/io-console/lib/ffi"

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
# Lay a fetched gem's `lib/` over the staged tree, refusing to replace a file
# already there, and copy its licenses to `LICENSE/<gem>/`.
lay_over() {
  local name="$1"
  shift
  local clashes
  clashes="$(cd "$work/$name/lib" && find . -type f | while read -r file; do
    if [[ -e "$staged/$file" ]]; then echo "$file"; fi
  done)"
  if [[ -n "$clashes" ]]; then
    echo "::error::ruby/$name would replace files already vendored:" >&2
    echo "$clashes" >&2
    exit 1
  fi
  cp -R "$work/$name/lib/." "$staged/"
  mkdir -p "$staged/LICENSE/$name"
  for file in "$@"; do
    cp "$work/$name/$file" "$staged/LICENSE/$name/$file"
  done
}
lay_over webrick "${WEBRICK_LICENSES[@]}"
lay_over json "${JSON_LICENSES[@]}"
lay_over io-console "${IO_CONSOLE_LICENSES[@]}"
cat > "$staged/UPSTREAM" <<EOF
Ruby's pure-Ruby standard library, vendored verbatim.

    upstream  $RUBY_REPO
    tag       $RUBY_TAG
    commit    $sha
    path      lib/ (flattened to this directory, so stdlib/ is a \$LOAD_PATH root)

Laid over it, also verbatim, the lib/ of three gems that are not in Ruby's
lib/: webrick, which left the standard library, and json and io-console,
default gems whose Ruby halves ruby/ruby keeps in ext/json/lib and
ext/io/console/lib. Their C halves are Spinel's own, in the repository's
lib/json/ext/ and lib/io/console.rb, embedded in the binary.

    upstream  $WEBRICK_REPO
    tag       $WEBRICK_TAG
    commit    $webrick_sha
    files     webrick.rb, webrick/

    upstream  $JSON_REPO
    tag       $JSON_TAG
    commit    $json_sha
    files     json.rb, json/

    upstream  $IO_CONSOLE_REPO
    tag       $IO_CONSOLE_TAG
    commit    $io_console_sha
    files     io/console/size.rb (not lib/ffi/, JRuby's backend)

Licenses are upstream's, in LICENSE/. LEGAL records the per-file terms for the
files here from ruby/ruby that are not under Ruby's own license; each gem's own
are in LICENSE/<gem>/.

Do not edit anything under stdlib/ by hand. CI diffs this tree against the tags
above and fails on any difference. To update, edit RUBY_TAG, WEBRICK_TAG or
JSON_TAG or IO_CONSOLE_TAG in scripts/vendor-stdlib.sh and run it.
EOF

if $check_only; then
  # `stdlib/` must match the staged tree exactly. Every difference is a failure,
  # which is stricter than "unexplained drift" and needs no allowlist to keep
  # honest. When a patch to a vendored file is genuinely needed, this is the
  # place to add one, with the reason next to it.
  if diff -r -q "$staged" "$stdlib"; then
    files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
    echo "stdlib/ matches ruby/ruby $RUBY_TAG, ruby/webrick $WEBRICK_TAG, ruby/json $JSON_TAG and ruby/io-console $IO_CONSOLE_TAG ($files .rb files)"
  else
    echo "::error::stdlib/ has drifted from ruby/ruby $RUBY_TAG ($sha), ruby/webrick $WEBRICK_TAG, ruby/json $JSON_TAG or ruby/io-console $IO_CONSOLE_TAG" >&2
    echo "run scripts/vendor-stdlib.sh and commit the result" >&2
    exit 1
  fi
else
  rm -rf "$stdlib"
  mv "$staged" "$stdlib"
  files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
  echo "vendored ruby/ruby $RUBY_TAG, ruby/webrick $WEBRICK_TAG, ruby/json $JSON_TAG and ruby/io-console $IO_CONSOLE_TAG into stdlib/ ($files .rb files)"
fi
