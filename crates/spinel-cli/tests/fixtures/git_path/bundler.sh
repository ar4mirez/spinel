#!/usr/bin/env bash
#
# Record what Bundler writes for the git and path source fixtures (#88),
# which `crates/spinel-cli/tests/git_path.rs` compares Spinel's lockfiles and
# Gemfiles with:
#
#   crates/spinel-cli/tests/fixtures/git_path/bundler.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/git_path/bundler
#
# `shared` 1.0.0 and 2.0.0 from `../update/gems/` are served on 127.0.0.1
# with a compact index written from their checksums, as the test serves
# them. A git repository is committed from `widget-1.0.0/` on `main`, with a
# fixed identity and date, and `gadget/` sits beside the project, where the
# Gemfile in `bundler/Gemfile` finds it. Then:
#
# - `bundle lock` writes `installed.lock`;
# - in a project whose Gemfile names no source, only `gadget` from its
#   path, `bundle lock` writes `nosource.lock`;
# - `widget-1.1.0/` is committed on top, and `bundle update widget` from
#   `installed.lock` writes `updated.lock` (`bundle update`, which `spinel
#   update` is: Bundler 4.0.20's `bundle lock --update widget` locks widget
#   1.1.0 at the old revision, where `bundle update widget` moves the
#   revision with it);
# - from `installed.lock`, with gadget's gemspec at 0.2.0, `bundle lock`
#   writes `bumped.lock`;
# - in a project whose Gemfile names only the source, `bundle add widget
#   --git <repository>` writes `added.Gemfile` and `added.lock`;
# - in `app/`, whose Gemfile is `gemspec`, `bundle lock` writes
#   `gemspec.lock`.
#
# What differs from run to run is spelled as a placeholder: the source as
# `@SOURCE@`, the repository's `file://` URL as `@REPO@`, and its two
# revisions as `@REVISION_1@` and `@REVISION_2@`. An opt-in tool, never run
# by CI.
#
# Written with Bundler 4.0.20 on Ruby 4.0.9 and git 2.56; Python 3 serves
# the files.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$here/bundler"
gems="$here/../update"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-git-path-bundler.XXXXXX")"
server=""
cleanup() {
  [[ -n "$server" ]] && kill "$server" 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

# The gem's `/info` dependencies field, from its gemspec.
deps() {
  sed -n 's/^  s.add_dependency "\([^"]*\)", "\([^"]*\)"$/\1:\2/p' "$1" | paste -sd, -
}

mkdir -p "$work/served/index/info" "$work/served/index/gems"
for gem in "$gems"/gems/shared-*.gem; do
  file="$(basename "$gem" .gem)"
  version="${file##*-}"
  cp "$gem" "$work/served/index/gems/"
  [[ -f "$work/served/index/info/shared" ]] || echo "---" >"$work/served/index/info/shared"
  echo "$version $(deps "$gems/src/$file/shared.gemspec")|checksum:$(sha256sum "$gem" | cut -d' ' -f1)" \
    >>"$work/served/index/info/shared"
done
{
  echo "created_at: 2026-01-01T00:00:00Z"
  echo "---"
  info="$work/served/index/info/shared"
  echo "shared $(grep -v '^---$' "$info" | cut -d' ' -f1 | paste -sd, -) $(md5sum "$info" | cut -d' ' -f1)"
} >"$work/served/index/versions"
python3 -u -m http.server --bind 127.0.0.1 0 --directory "$work/served" >"$work/server.log" 2>&1 &
server=$!
port=""
for _ in $(seq 50); do
  port="$(sed -n 's/.* port \([0-9]*\) .*/\1/p' "$work/server.log" | head -1)"
  [[ -n "$port" ]] && break
  sleep 0.1
done
[[ -n "$port" ]] || { cat "$work/server.log"; exit 2; }
source_url="http://127.0.0.1:$port/index"

# Commit `$1/` over what the repository has, as the test commits it: file
# modes fixed, a fixed identity, and a date a day later for each commit.
repo="$work/repo"
repo_url="file://$repo"
commits=0
commit() {
  mkdir -p "$repo"
  (
    cd "$repo"
    [[ -d .git ]] || git -c init.defaultBranch=main init -q
    git rm -rq --ignore-unmatch . >/dev/null
    cp -R "$here/$1/." .
    find . -path ./.git -prune -o -type f -exec chmod 0644 {} +
    export GIT_AUTHOR_NAME=Spinel GIT_AUTHOR_EMAIL=spinel@example.invalid
    export GIT_COMMITTER_NAME=Spinel GIT_COMMITTER_EMAIL=spinel@example.invalid
    export GIT_AUTHOR_DATE="2026-01-0$((commits + 1))T00:00:00Z"
    export GIT_COMMITTER_DATE="$GIT_AUTHOR_DATE"
    git add -A
    git -c commit.gpgsign=false commit -q -m "$1"
  )
  commits=$((commits + 1))
}

bundle_in() {
  local app="$1"
  shift
  (
    cd "$app"
    env -i PATH="$PATH" HOME="$work/home" \
      BUNDLE_GEMFILE="$app/Gemfile" \
      BUNDLE_USER_HOME="$work/bundle" \
      BUNDLE_APP_CONFIG="$app/.bundle" \
      BUNDLE_PATH="$app/vendor" \
      GEM_HOME="$work/gem_home" GEM_PATH="$work/gem_home" \
      bundle "$@"
  )
}
revision_1=""
revision_2=""
record() {
  local text
  text="$(sed -e "s|$source_url|@SOURCE@|g" -e "s|$repo_url|@REPO@|g" "$1")"
  [[ -n "$revision_1" ]] && text="${text//$revision_1/@REVISION_1@}"
  [[ -n "$revision_2" ]] && text="${text//$revision_2/@REVISION_2@}"
  printf '%s\n' "$text" >"$out/$2"
}

commit widget-1.0.0
revision_1="$(git -C "$repo" rev-parse HEAD)"
app="$work/app"
mkdir -p "$app"
cp -R "$here/gadget" "$work/gadget"
sed -e "s|@SOURCE@|$source_url|" -e "s|@REPO@|$repo_url|" "$out/Gemfile" >"$app/Gemfile"
bundle_in "$app" lock >/dev/null
record "$app/Gemfile.lock" installed.lock
cp "$app/Gemfile.lock" "$work/installed.lock"

nosource="$work/nosource"
mkdir -p "$nosource"
printf 'gem "gadget", path: "../gadget"\n' >"$nosource/Gemfile"
bundle_in "$nosource" lock >/dev/null
record "$nosource/Gemfile.lock" nosource.lock

commit widget-1.1.0
revision_2="$(git -C "$repo" rev-parse HEAD)"
bundle_in "$app" update widget >/dev/null
record "$app/Gemfile.lock" updated.lock

cp "$work/installed.lock" "$app/Gemfile.lock"
sed -i 's/s.version = "0.1.0"/s.version = "0.2.0"/' "$work/gadget/gadget.gemspec"
bundle_in "$app" lock >/dev/null
record "$app/Gemfile.lock" bumped.lock

added="$work/added"
mkdir -p "$added"
printf 'source "%s"\n' "$source_url" >"$added/Gemfile"
bundle_in "$added" add widget --git "$repo_url" >/dev/null
record "$added/Gemfile" added.Gemfile
record "$added/Gemfile.lock" added.lock

gemspec_app="$work/app-gemspec"
cp -R "$here/app" "$gemspec_app"
sed -i "s|@SOURCE@|$source_url|" "$gemspec_app/Gemfile"
bundle_in "$gemspec_app" lock >/dev/null
record "$gemspec_app/Gemfile.lock" gemspec.lock
