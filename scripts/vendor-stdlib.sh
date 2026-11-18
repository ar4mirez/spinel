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
# so the check covers them. Beside them, `libexec/` holds upstream's
# `libexec/bundle`, Bundler's executable (#71): the binary extracts the
# library to `lib/` and it to `libexec/` next to it, as ruby/ruby has them,
# where `Bundler.setup` looks for the executable it records in
# `BUNDLE_BIN_PATH` (`crates/spinel-cli/build.rs`).
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
# `prism` (#837) is laid over differently: it replaces the copy in Ruby's
# `lib/`. Its C half is the Prism Spinel's own parser links (the ruby-prism
# crate, `crates/spinel-parse`), and its Ruby half reads that library's
# serialized trees, which `Prism::Serialize` accepts only from its own
# release, byte for byte. So the Ruby half is the gem at the crate's
# release, from the packaged gem on rubygems.org rather than a git tag,
# because `node.rb`, `serialize.rb` and the other generated files exist only
# in the package. Its `prism/ffi.rb`, the backend for engines without the C
# extension, is left out: Spinel's C half is `lib/prism/prism.rb`, embedded,
# and `prism.rb` is patched to load it (see `patch_prism` below).
#
# `fiddle` (#105) is a bundled gem, so it is in neither Ruby's `lib/` nor its
# `ext/`; its Ruby half comes from the packaged gem at the release Ruby 4.0
# bundles, as prism's does, with `fiddle/ffi_backend.rb`, the backend for
# engines without the C extension, left out. Its C half is Spinel's
# `lib/fiddle/spinel.rb` over `Spinel::FFI` (`lib/spinel/ffi.rb`), embedded,
# and `fiddle.rb` and `fiddle/cparser.rb` are patched (see `patch_fiddle`).
#
# To bump: edit RUBY_TAG, WEBRICK_TAG, JSON_TAG, IO_CONSOLE_TAG,
# PRISM_VERSION (with PRISM_SHA256, and the ruby-prism crate in
# `crates/spinel-parse/Cargo.toml` to the same release) or FIDDLE_VERSION
# (with FIDDLE_SHA256), run this script, commit `stdlib/` in its own commit.

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

# The prism gem, at the release of the ruby-prism crate Spinel parses with,
# and the package's SHA-256 as rubygems.org publishes it.
PRISM_VERSION="1.9.0"
PRISM_SHA256="7b530c6a9f92c24300014919c9dcbc055bf4cdf51ec30aed099b06cd6674ef85"
PRISM_URL="https://rubygems.org/downloads/prism-$PRISM_VERSION.gem"

# The fiddle Ruby 4.0 bundles (gems/bundled_gems at RUBY_TAG), and the
# package's SHA-256 as rubygems.org publishes it.
FIDDLE_VERSION="1.1.8"
FIDDLE_SHA256="7fa8ee3627271497f3add5503acdbc3f40b32f610fc1cf49634f083ef3f32eee"
FIDDLE_URL="https://rubygems.org/downloads/fiddle-$FIDDLE_VERSION.gem"

# Bundler's executable, copied to `stdlib/libexec/`.
LIBEXEC_FILES=(bundle)

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
    printf '/libexec/%s\n' "${LIBEXEC_FILES[@]}"
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

# prism is MIT-licensed. A `.gem` is a tar of a gzipped `data.tar.gz`; only
# its `lib/`, `prism.gemspec` and license are wanted.
PRISM_LICENSES=(LICENSE.md)

# Fetch a packaged gem from rubygems.org to `$work/<name>-gem/<name>.gem` and
# stop unless its SHA-256 is the one pinned.
fetch_package() {
  local name="$1" url="$2" pinned="$3" got
  mkdir -p "$work/$name-gem" "$work/$name"
  curl -fsSL -o "$work/$name-gem/$name.gem" "$url"
  if command -v sha256sum >/dev/null; then
    got="$(sha256sum "$work/$name-gem/$name.gem" | cut -d' ' -f1)"
  else
    got="$(shasum -a 256 "$work/$name-gem/$name.gem" | cut -d' ' -f1)"
  fi
  if [[ "$got" != "$pinned" ]]; then
    echo "::error::$(basename "$url") is $got, not $pinned" >&2
    exit 1
  fi
}

echo "fetching prism $PRISM_VERSION from rubygems.org"
fetch_package prism "$PRISM_URL" "$PRISM_SHA256"
tar -xf "$work/prism-gem/prism.gem" -C "$work/prism-gem" data.tar.gz
tar -xzf "$work/prism-gem/data.tar.gz" -C "$work/prism" lib prism.gemspec "${PRISM_LICENSES[@]}"
rm "$work/prism/lib/prism/ffi.rb"
# ruby/ruby keeps the gemspec beside the library, in `lib/prism/`.
mv "$work/prism/prism.gemspec" "$work/prism/lib/prism/prism.gemspec"

# Replace the one line of `file` that is exactly `from` with `to`. The script
# stops if the line is not there exactly once, so an upstream change to it is
# looked at rather than patched blind.
patch_line() {
  local file="$1" from="$2" to="$3"
  if [[ "$(grep -cxF "$from" "$file")" != 1 ]]; then
    echo "::error::$file no longer has the line a patch replaces: $from" >&2
    exit 1
  fi
  local patched="$file.patched"
  while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ "$line" == "$from" ]]; then printf '%s\n' "$to"; else printf '%s\n' "$line"; fi
  done < "$file" > "$patched"
  mv "$patched" "$file"
}

# `prism.rb` loads the C extension only on CRuby (`RUBY_ENGINE == "ruby"`) and
# the FFI backend everywhere else; on Spinel the extension is
# `lib/prism/prism.rb`, embedded, so Spinel takes the extension's branch.
# Upstream's own words, an engine added to its condition.
patch_prism() {
  patch_line "$1" \
    'if RUBY_ENGINE == "ruby" and !ENV["PRISM_FFI_BACKEND"]' \
    'if (RUBY_ENGINE == "ruby" || RUBY_ENGINE == "spinel") and !ENV["PRISM_FFI_BACKEND"]'
}
patch_prism "$work/prism/lib/prism.rb"

# fiddle is under Ruby's license or the 2-clause BSDL, in one file.
FIDDLE_LICENSES=(LICENSE.txt)
echo "fetching fiddle $FIDDLE_VERSION from rubygems.org"
fetch_package fiddle "$FIDDLE_URL" "$FIDDLE_SHA256"
tar -xf "$work/fiddle-gem/fiddle.gem" -C "$work/fiddle-gem" data.tar.gz
tar -xzf "$work/fiddle-gem/data.tar.gz" -C "$work/fiddle" lib "${FIDDLE_LICENSES[@]}"
rm "$work/fiddle/lib/fiddle/ffi_backend.rb"

# Two patches to fiddle, each the least that works:
#
# - `fiddle.rb` requires `fiddle.so` on CRuby and the FFI backend elsewhere.
#   On Spinel the C half is `fiddle/spinel`, embedded, and `last_error` is
#   kept as `fiddle.so` keeps it, so Spinel takes CRuby's branches.
# - `fiddle/cparser.rb` reads `$3` (and `$2`) after calling `parse_ctype`,
#   whose own matches CRuby keeps out of the caller's `$~`. Spinel's `$~` is
#   one per heap, not per frame (the ponytail on `last_match` in
#   `spinel-vm/src/heap.rs`), so the count is read first. Drop this patch
#   when `$~` is frame-local.
patch_fiddle() {
  local lib="$1"
  patch_line "$lib/fiddle.rb" \
    "if RUBY_ENGINE == 'ruby'" \
    "if RUBY_ENGINE == 'ruby' || RUBY_ENGINE == 'spinel'"
  patch_line "$lib/fiddle.rb" \
    "  require 'fiddle.so'" \
    "  require RUBY_ENGINE == 'spinel' ? 'fiddle/spinel' : 'fiddle.so'"
  patch_line "$lib/fiddle.rb" \
    "  if RUBY_ENGINE != 'ruby' # FFI backend" \
    "  if RUBY_ENGINE != 'ruby' && RUBY_ENGINE != 'spinel' # FFI backend"
  patch_line "$lib/fiddle/cparser.rb" \
    '          tys.push([parse_ctype($1.strip, tymap), $3.to_i])' \
    '          count = $3.to_i; tys.push([parse_ctype($1.strip, tymap), count])'
  patch_line "$lib/fiddle/cparser.rb" \
    '          tys.push([parse_ctype($1.strip, tymap), $2.to_i])' \
    '          count = $2.to_i; tys.push([parse_ctype($1.strip, tymap), count])'
}
patch_fiddle "$work/fiddle/lib"

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
if [[ -e "$staged/libexec" ]]; then
  echo "::error::ruby/ruby's lib/ has a libexec/, which would clash with Bundler's executable" >&2
  exit 1
fi
mkdir -p "$staged/libexec"
for file in "${LIBEXEC_FILES[@]}"; do
  cp -p "$work/ruby/libexec/$file" "$staged/libexec/$file"
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
# Ruby's own prism goes first: the gem's replaces it rather than mixing two
# releases' files (see PRISM_VERSION).
rm -rf "$staged/prism.rb" "$staged/prism"
lay_over prism "${PRISM_LICENSES[@]}"
lay_over fiddle "${FIDDLE_LICENSES[@]}"
cat > "$staged/UPSTREAM" <<EOF
Ruby's pure-Ruby standard library, vendored verbatim.

    upstream  $RUBY_REPO
    tag       $RUBY_TAG
    commit    $sha
    path      lib/ (flattened to this directory, so stdlib/ is a \$LOAD_PATH root)
              libexec/bundle (to libexec/, Bundler's executable,
              extracted beside the library rather than in it)

Laid over it, also verbatim, the lib/ of three gems that are not in Ruby's
lib/: webrick, which left the standard library, and json and io-console,
default gems whose Ruby halves ruby/ruby keeps in ext/json/lib and
ext/io/console/lib. Their C halves are Spinel's own, in the repository's
lib/json/ext/ and lib/io/console.rb, embedded in the binary.

Also laid over it, the bundled gem fiddle's lib/, from the packaged gem at
the release Ruby bundles. Its C half is Spinel's lib/fiddle/spinel.rb, over
Spinel::FFI (lib/spinel/ffi.rb), embedded in the binary; fiddle/ffi_backend.rb,
the FFI backend, is left out. Two files are patched: fiddle.rb takes CRuby's
branches when RUBY_ENGINE is "spinel", requiring fiddle/spinel in place of
fiddle.so, and fiddle/cparser.rb reads an array member's count before it
calls parse_ctype, since Spinel's $~ is not yet frame-local.

In place of Ruby's own prism.rb and prism/, the prism gem's lib/ at the
release of the Prism library Spinel parses with, which is the C half its
Ruby half reads serialized trees from (the repository's lib/prism/prism.rb,
embedded in the binary). Its prism.gemspec is in prism/, where ruby/ruby
keeps it. One file is patched: prism.rb loads the C extension when
RUBY_ENGINE is "spinel" as well as "ruby", rather than prism/ffi.rb, the
FFI backend, which is left out.

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

    upstream  $PRISM_URL
    sha256    $PRISM_SHA256
    files     prism.rb (patched), prism/ (not prism/ffi.rb, the FFI backend)

    upstream  $FIDDLE_URL
    sha256    $FIDDLE_SHA256
    files     fiddle.rb (patched), fiddle/ (cparser.rb patched; not
              fiddle/ffi_backend.rb, the FFI backend)

Licenses are upstream's, in LICENSE/. LEGAL records the per-file terms for the
files here from ruby/ruby that are not under Ruby's own license; each gem's own
are in LICENSE/<gem>/.

Do not edit anything under stdlib/ by hand. CI diffs this tree against the tags
above and fails on any difference. To update, edit RUBY_TAG, WEBRICK_TAG,
JSON_TAG, IO_CONSOLE_TAG, PRISM_VERSION or FIDDLE_VERSION in
scripts/vendor-stdlib.sh and run it.
EOF

if $check_only; then
  # `stdlib/` must match the staged tree exactly. Every difference is a failure,
  # which is stricter than "unexplained drift" and needs no allowlist to keep
  # honest. When a patch to a vendored file is genuinely needed, this is the
  # place to add one, with the reason next to it.
  if diff -r -q "$staged" "$stdlib"; then
    files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
    echo "stdlib/ matches ruby/ruby $RUBY_TAG, ruby/webrick $WEBRICK_TAG, ruby/json $JSON_TAG, ruby/io-console $IO_CONSOLE_TAG, prism $PRISM_VERSION and fiddle $FIDDLE_VERSION ($files .rb files)"
  else
    echo "::error::stdlib/ has drifted from ruby/ruby $RUBY_TAG ($sha), ruby/webrick $WEBRICK_TAG, ruby/json $JSON_TAG, ruby/io-console $IO_CONSOLE_TAG, prism $PRISM_VERSION or fiddle $FIDDLE_VERSION" >&2
    echo "run scripts/vendor-stdlib.sh and commit the result" >&2
    exit 1
  fi
else
  rm -rf "$stdlib"
  mv "$staged" "$stdlib"
  files="$(find "$stdlib" -name '*.rb' | wc -l | tr -d ' ')"
  echo "vendored ruby/ruby $RUBY_TAG, ruby/webrick $WEBRICK_TAG, ruby/json $JSON_TAG, ruby/io-console $IO_CONSOLE_TAG, prism $PRISM_VERSION and fiddle $FIDDLE_VERSION into stdlib/ ($files .rb files)"
fi
