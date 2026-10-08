#!/usr/bin/env bash
#
# Run ruby/spec against Spinel, with mspec running on Spinel (#145).
#
#   scripts/spec.sh                        every spec in the corpus
#   scripts/spec.sh core/array             one directory
#   scripts/spec.sh language/if_spec.rb    one file
#   scripts/spec.sh --list core/array      one record per example instead of counts
#   scripts/spec.sh --blocked=0 language   rank every blocking reason, not the top 20
#   scripts/spec.sh --table                bench/spec-status.md, as spec-status.sh writes it
#   scripts/spec.sh --tagged core/array    run only the examples tagged `fails`
#
# Paths are relative to `spec/ruby`, which is where the ruby/spec submodule is
# checked out, so the argument reads the way ruby/spec's own directories do. A
# path that is not in the corpus but exists as given is used as given, so
# `spec/ruby/core/array` works too.
#
# An example passes, fails — an expectation was not met — or is blocked:
# Spinel cannot run it yet (`spec/spinel/runner.rb` says how). The run ends by
# ranking what blocked examples, most first, which is how the next slice gets
# chosen. These counts are the project's progress bar.
#
# The corpus is split into mspec processes of a few files each, run in
# parallel, and their records are merged by `scripts/spec-report.sh`. A
# process is also the unit a crash takes down: one that stops without
# reporting is named, with the end of its stderr, and the run fails. `--tagged` is how a stale tag is found: a tagged example
# that passes is a tag to delete.
#
# `--list` prints `kind<TAB>file<TAB>description<TAB>detail` per record, and
# first `chunk<TAB>N<TAB>file` for each file, naming the process it ran in.
# That is what `scripts/verify-passes.rb` reads.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$repo_root/spec/ruby"
# `MSPEC_DIR` points at another mspec checkout, for a machine where the
# submodule cannot be fetched.
mspec="${MSPEC_DIR:-$repo_root/spec/mspec}"

# A submodule that was never initialised looks exactly like a corpus with no
# specs in it. Say which one it is, and how to fix it, rather than reporting a
# clean run over nothing.
if [[ ! -e "$corpus/spec_helper.rb" ]]; then
  echo "spec.sh: ruby/spec is not checked out at spec/ruby" >&2
  echo "         git submodule update --init spec/ruby" >&2
  exit 2
fi
if [[ ! -e "$mspec/bin/mspec-run" ]]; then
  echo "spec.sh: mspec is not checked out at spec/mspec" >&2
  echo "         git submodule update --init spec/mspec" >&2
  exit 2
fi

mode=report
report_flags=()
tagged=
paths=()
for argument in "$@"; do
  case "$argument" in
    --list) mode=list ;;
    --table) mode=table ;;
    --tagged) tagged=1 ;;
    --blocked=*) report_flags+=("$argument") ;;
    -*)
      echo "spec.sh: unknown flag: $argument" >&2
      exit 2
      ;;
    *)
      # The corpus first: `core` is also this repository's own `core/`.
      if [[ -e "$corpus/$argument" ]]; then
        paths+=("$corpus/$argument")
      elif [[ -e "$argument" ]]; then
        paths+=("$(cd "$(dirname "$argument")" && pwd)/$(basename "$argument")")
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

# Every spec file under the paths, in mspec's order. `fixtures/` and `shared/`
# hold Ruby that specs load, never a `*_spec.rb`.
files=()
while IFS= read -r file; do
  files+=("$file")
done < <(for path in "${paths[@]}"; do
  if [[ -f "$path" ]]; then echo "$path"; else find "$path" -name '*_spec.rb'; fi
done | LC_ALL=C sort -u)
if [[ ${#files[@]} -eq 0 ]]; then
  echo "spec.sh: no spec files under ${paths[*]}" >&2
  exit 2
fi

# Built quietly so the report is the only thing on stdout, but not silently: a
# compile error still has to reach the terminal.
cargo build --release -p spinel-cli --quiet
spinel="$repo_root/target/release/spinel"

scratch="$(mktemp -d)"
# Where the specs keep their files (mspec's `tmp`): a directory per process,
# so two of them cannot hand out the same name, under `target/` so it is on
# the repository's file system and ignored by git, and removed with the run.
# An example that blocks part way never reaches its own cleanup, so without
# this every run leaves a `rubyspec_temp/` behind in the working directory.
spec_tmp="$repo_root/target/spec-tmp/$$"
mkdir -p "$spec_tmp"
trap 'rm -rf "$scratch" "$spec_tmp"' EXIT

# The files are split into processes of `SPEC_CHUNK` (default 16), run
# `SPEC_JOBS` at a time (default every core). Chunks, not directories: a slow
# directory — `core/io`, `core/string` — would otherwise set the wall time on
# its own. Process N reads its files from `N.files` and writes its records to
# `N.out`, its stderr — warnings, and whatever a crash says — to `N.err`, and
# its exit status to `N.status`. No corpus path has a space in it, which is
# what lets the file list be word-split.
chunk="${SPEC_CHUNK:-16}"
units=()
for ((at = 0; at < ${#files[@]}; at += chunk)); do
  printf '%s\n' "${files[@]:at:chunk}" > "$scratch/${#units[@]}.files"
  units+=("${files[$at]}")
done
export RUBY_EXE="$spinel" SPINEL_SPEC_LIST=1
if [[ -n "$tagged" ]]; then export SPINEL_SPEC_TAGGED=1; fi
for index in "${!units[@]}"; do
  printf '%s\0' "$index"
done | xargs -0 -n 1 -P "${SPEC_JOBS:-$(getconf _NPROCESSORS_ONLN)}" \
  sh -c 'status=0; SPEC_TEMP_DIR="$4/$5" "$0" run "$1" -B "$2" $(cat "$3/$5.files") > "$3/$5.out" 2> "$3/$5.err" || status=$?; echo "$status" > "$3/$5.status"' \
  "$spinel" "$mspec/bin/mspec-run" "$repo_root/spec/spinel.mspec" "$scratch" "$spec_tmp"


# mspec exits 0, or 1 when an example failed. Anything else, or no records at
# all, is a process that stopped before it could report.
stopped=0
for index in "${!units[@]}"; do
  status="$(cat "$scratch/$index.status")"
  if [[ "$status" != 0 && "$status" != 1 ]] || ! grep -aq '^LIST' "$scratch/$index.out"; then
    stopped=1
    echo "spec.sh: the process starting at ${units[$index]#"$corpus"/} stopped before reporting (exit $status):" >&2
    tail -n 5 "$scratch/$index.err" | sed 's/^/    /' >&2
  fi
done

status=0
case "$mode" in
  list)
    # Which files shared a process, so `verify-passes.rb` can run them on Ruby
    # the same way: state one file leaves behind is visible to the next.
    for index in "${!units[@]}"; do
      sed "s|^$repo_root/|chunk	$index	|" "$scratch/$index.files"
    done
    cat "$scratch"/*.out | grep -a '^LIST' | cut -f 2-
    ;;
  table) "$repo_root/scripts/spec-report.sh" --table "$scratch"/*.out || status=$? ;;
  report)
    "$repo_root/scripts/spec-report.sh" ${report_flags[@]+"${report_flags[@]}"} "$scratch"/*.out \
      || status=$?
    ;;
esac
[[ $stopped -ne 0 ]] && exit 1
exit "$status"
