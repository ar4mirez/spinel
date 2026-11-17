#!/usr/bin/env bash
#
# Record what Bundler 4.0.20 writes and prints for the Gemfiles
# `crates/spinel-cli/tests/private_source.rs` installs (#89), against
# `server.py` serving this directory's indexes on 127.0.0.1:
#
#   crates/spinel-cli/tests/fixtures/private_source/bundler.sh <scratch dir>
#
# It writes `expected/`: `compact.lock`, a private source behind HTTP Basic
# credentials given in its URL; `legacy.lock`, a source with no compact index
# once `bundle install` has fetched its gems; `scoped.lock`, a public global
# source and a private one scoped to `rackup`, its credentials in
# `BUNDLE_127__0__0__1`; and `bad_credentials.txt` and
# `no_credentials.txt`, what `bundle lock` prints for a credential refused
# and for none given. A source is written `@SOURCE@` (and the scoped one
# `@PRIVATE@`) in place of its port. The scratch directory must not exist;
# nothing is deleted.
#
# Opt-in, never run by CI: it needs Ruby, Bundler 4.0.20 and Python 3.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch="$1"
mkdir "$scratch"
cd "$scratch"
export BUNDLE_USER_HOME="$scratch/bundle-home" BUNDLE_PATH="$scratch/bundle-path" GEM_HOME="$scratch/gem-home"
unset BUNDLE_GEMFILE BUNDLE_APP_CONFIG BUNDLE_FROZEN

python3 "$here/server.py" compact private.port user:s3cret-pw &
private=$!
python3 "$here/server.py" compact public.port &
public=$!
python3 "$here/server.py" legacy legacy.port user:s3cret-pw &
legacy=$!
trap 'kill $private $public $legacy' EXIT
for f in private.port public.port legacy.port; do
  while [ ! -s "$f" ]; do sleep 0.1; done
done
p="$(cat private.port)" q="$(cat public.port)" l="$(cat legacy.port)"
mkdir -p "$here/expected" compact legacy scoped bad none

printf 'source "http://user:s3cret-pw@127.0.0.1:%s"\ngem "rack-session"\n' "$p" >compact/Gemfile
(cd compact && bundle lock >/dev/null)
sed "s|http://127.0.0.1:$p/|@SOURCE@/|" compact/Gemfile.lock >"$here/expected/compact.lock"

printf 'source "http://user:s3cret-pw@127.0.0.1:%s"\ngem "rack-session"\n' "$l" >legacy/Gemfile
(cd legacy && bundle install >/dev/null)
sed "s|http://127.0.0.1:$l/|@SOURCE@/|" legacy/Gemfile.lock >"$here/expected/legacy.lock"

printf 'source "http://localhost:%s"\ngem "rack"\nsource "http://127.0.0.1:%s" do\n  gem "rackup"\nend\n' "$q" "$p" >scoped/Gemfile
(cd scoped && BUNDLE_127__0__0__1=user:s3cret-pw bundle lock >/dev/null)
sed "s|http://localhost:$q/|@SOURCE@/|; s|http://127.0.0.1:$p/|@PRIVATE@/|" scoped/Gemfile.lock >"$here/expected/scoped.lock"

printf 'source "http://user:wrong-pw@127.0.0.1:%s"\ngem "rack-session"\n' "$p" >bad/Gemfile
(cd bad && { bundle lock 2>&1 >/dev/null; echo "exit $?"; } | grep -v '^Writing lockfile' >"$here/expected/bad_credentials.txt") || true
printf 'source "http://127.0.0.1:%s"\ngem "rack-session"\n' "$p" >none/Gemfile
(cd none && { bundle lock 2>&1 >/dev/null; echo "exit $?"; } | grep -v '^Writing lockfile' >"$here/expected/no_credentials.txt") || true
