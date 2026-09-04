#!/usr/bin/env bash
#
# Run ruby/spec against Spinel.
#
#   scripts/spec.sh                        every spec in the corpus
#   scripts/spec.sh core/array             one directory
#   scripts/spec.sh language/if_spec.rb    one file
#   scripts/spec.sh --list core/array      print example names instead of counts
#
# Paths are relative to `spec/ruby`, which is where the ruby/spec submodule is
# checked out, so the argument reads the way ruby/spec's own directories do. A
# path that exists as given is used as given, so `spec/ruby/core/array` works too.
#
# No example passes yet: this build has no VM, so every example is reported
# blocked. The counts are still real, and they are the project's progress bar.
# `spec/harness/` is deleted when mspec runs on Spinel, at the end of phase 2.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$repo_root/spec/ruby"

# A submodule that was never initialised looks exactly like a corpus with no
# specs in it. Say which one it is, and how to fix it, rather than reporting a
# clean run over nothing.
if [[ ! -e "$corpus/spec_helper.rb" ]]; then
  echo "spec.sh: ruby/spec is not checked out at spec/ruby" >&2
  echo "         git submodule update --init spec/ruby" >&2
  exit 2
fi

# Flags go to the harness untouched; bare words are corpus paths.
flags=()
paths=()
for argument in "$@"; do
  case "$argument" in
    -*) flags+=("$argument") ;;
    *)
      if [[ -e "$argument" ]]; then
        paths+=("$argument")
      elif [[ -e "$corpus/$argument" ]]; then
        paths+=("$corpus/$argument")
      else
        echo "spec.sh: no such spec file or directory: $argument" >&2
        echo "         looked for it here and under spec/ruby/" >&2
        exit 2
      fi
      ;;
  esac
done
# No path given means the whole corpus, which is the same default `mspec` has.
[[ ${#paths[@]} -eq 0 ]] && paths=("$corpus")

# Built quietly so the report is the only thing on stdout, but not silently: a
# compile error still has to reach the terminal.
cargo build --release -p spec-harness --quiet

exec "$repo_root/target/release/spec-harness" "${flags[@]}" "${paths[@]}"
