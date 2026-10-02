# PRD 0052 — `require`, `load`, and the load path

Issue: [#39](https://github.com/ar4mirez/spinel/issues/39) · Phase 2 (pulled
forward from Phase 3 for #145) · `area:engine`

## Objective

Loading other files: `require`, `require_relative`, `load`, `$LOAD_PATH` and
`$LOADED_FEATURES`. mspec is a library of files that require each other, so
this is #145's second prerequisite, after string `eval` (#38).

## Baseline

`core/kernel` 346 / 1277, `core/file` 0 / 622, `language` 1611 / 2464. Corpus
6858, 0 failed.

## Design

**One primitive.** `__load_file__(path, wrap, self)` reads, parses (through
the parser hook #38 added), compiles and pushes a frame that runs the file's
top level: `self` is `main`, the scope is the top level, and a `def` is
private, as in the main script. Everything else is Ruby in `core/load.rb`.

**`require` is Ruby.** A name without `.rb` gets it; a native-extension
suffix is a `LoadError`; an absolute or `./`-relative name is expanded and
anything else is searched along `$LOAD_PATH`. The path is recorded in
`$LOADED_FEATURES` *before* the file runs, so a file that requires itself
answers false instead of recursing, and removed again if it raises.
`require_relative` resolves against the caller's file — from its binding —
and refuses an `eval` with no file name, as CRuby does. `load(file, true)`
runs the file under an anonymous module, with a `main` whose `include` lands
in that module.

**The runtime's globals are read-only.** `$:`, `$-I` and `$"` are aliases of
`$LOAD_PATH` and `$LOADED_FEATURES` through #166's global table, and those,
`$-a`, `$-l`, `$-p` and `$?` refuse assignment with CRuby's `NameError`.

**`File` and `Dir`, the path half.** `File.join`, `basename`, `dirname`,
`extname`, `expand_path`, `absolute_path`, `realpath`, `exist?`, `file?`,
`directory?`, `symlink?`, `read`, `File::Constants` from the target's libc,
`Dir.pwd` and `Dir.chdir` — Ruby over five file system primitives, each
answering an errno for the Ruby side to raise as `SystemCallError`. `IO` exists
as their superclass; reading and writing through one is #41.

**`defined?` answers `nil` again.** Since #10 a name the heap had never seen
was refused, because a fresh heap could not tell "undefined" from "in a file
not loaded yet". A program can now load what it needs, so a miss is `nil`,
except in a heap marked *partial*: `spec/harness` marks one where a fixture
could not be compiled or raised part way.

## The harness

Fixtures now run their own `require_relative` lines. The harness records every
fixture it preloaded in `$LOADED_FEATURES` first, so those lines are the
no-op they are in Ruby, and it parses spec and fixture files under their
absolute paths, as mspec loads them, so `__FILE__` and `__dir__` do not depend
on where the runner started. Fixtures that used to stop at their first
`require_relative` now run to the end in every example's heap, which is most
of the corpus run's time going from about 260s to about 340s.

## Delta

| directory | before | after |
|---|---:|---:|
| `core/kernel` | 346 / 1277 | **384 / 1277** |
| `core/file` | 0 / 622 | **75 / 622** |
| `language` | 1611 / 2464 | **1626 / 2464** |
| `core/module` | 460 / 1021 | **466 / 1021** |
| `core/io` | 0 / 1095 | **5 / 1095** |
| **corpus** | 6858 | **7003** |

0 failed; all passes re-run on ruby 4.0.7 agree. Seven `predefined_spec.rb`
tags removed: the read-only globals.

## What is left of #39

`autoload` — a constant miss that loads a file and retries — and the shared
`require` and `load` groups, which the harness runs without their
`it_behaves_like` argument and mspec will run with it (#145). `require` is not
yet thread-safe because there are no threads (#45).
