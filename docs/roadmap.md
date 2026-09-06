# Roadmap

Each phase ends with something runnable. Each bullet is one slice: one AI session, with a named check that proves it. Engine slices are measured in ruby/spec files that newly pass; the check names the directory. No dates. The progress bar is `bench/spec-status.md`.

Every bullet below is tracked as a GitHub issue, one milestone per phase: [milestones](https://github.com/ar4mirez/spinel/milestones). Issues carry a priority (`P0` critical path, `P1` needed for the milestone, `P2` can slip), a size (`size:S` one session, `size:M` two or three, `size:L` more), and an `area:` label. When a slice here and its issue disagree, fix both in the same PR.

## Phase 0: skeleton

- Cargo workspace with the crates from architecture.md, CI on macOS arm64 and Linux x64, `spinel --version`. *Check:* green CI.
- `spinel-ast` types covering Prism's node set; `spinel-parse` lowering; `spinel parse file.rb` prints the tree. *Check:* `spinel parse <dir>` lowers a ruby/spec and pure-Ruby-stdlib corpus with no "unhandled node" error. Both corpora are in the tree — `stdlib/` vendored, `spec/ruby/` a submodule — so the sweep reads them directly and nothing is cloned at a floating pin.
- Vendor the pure-Ruby stdlib under `stdlib/` — ruby/ruby's `lib/` at a pinned tag, with its license files. Not a `git subtree`: that copies a whole repository, and this is one directory of one. *Check:* `stdlib/` present, license present, CI job diffs it against the upstream tag.
- ruby/spec submodule at the branch matching the target language version, `spec/harness/` Rust runner: `describe`/`it` discovery out of the syntax tree, and the `ruby_version_is`/`platform_is` guards evaluated against the target. Matchers wait for phase 1 — `should ==` cannot compare anything without a VM, and ruby/spec spells the other one `should.raise`, not `should_raise`. *Check:* the harness runs `language/if_spec.rb` and reports, even though every example is blocked.

## Phase 1: a VM that runs `language/`

- `Value`, tagged fixnum/flonum/symbol/special constants, `Heap` with mark-sweep GC and size classes, `HandleScope`. *Check:* allocation stress test survives 10M objects under a forced GC every 1k allocations.
- Bootstrap classes, method tables, ancestor chains, and the global method cache. *Check:* Rust unit tests for `include`/`prepend` ordering against ruby/spec's documented cases — measured from CRuby by a script CI re-runs, not written by hand.
- Shapes: hidden-class instance variables and the per-heap shape tree. *Check:* two objects that set the same names in the same order share a shape, and in a different order do not. **Done:** the shape id lives in the header bytes #7 reserved, the values behind one slot the way an `Array`'s elements already are; a class's table id, an exception's message, and a `Hash`'s three slots all became ordinary ivars, and `attr_accessor` addresses a name rather than a slot. `language/` 626 to 667, the corpus 1,144 to 1,243, and the largest blocker in the corpus — 8,829 examples, 39% of everything blocked — is gone from the ranking entirely.
- Per-class serials, to replace the one-per-table serial the method cache ships with, and the subclass list they need. *Check:* defining a method on one class leaves another class's cached lookup in place.
- Bytecode + compiler for literals, locals, control flow, `while`/`until`, `case/when`. *Check:* `language/{if,unless,while,until,case}_spec.rb`. **Done for the remaining literals:** hash, range — including the beginless and endless forms — and a splat in an array literal, plus multiple assignment, a destructuring block parameter, and string interpolation (#157, #154). Each lowers to sends rather than to a new opcode, so `core/hash.rb`, `core/range.rb` and `core/array.rb` own the semantics; `Range` is a new core class holding a begin, an end and an exclusive flag, and its library is still #23's.
- Method definition and the full calling convention, `yield`, blocks, procs, lambdas. *Check:* `language/{def,block,lambda,proc,yield,send}_spec.rb`.
- Exceptions, `ensure`, `retry`, `throw/catch`, non-local `break`/`return`. *Check:* `language/{rescue,ensure,throw,break,return,next,redo}_spec.rb`.
- Constants, modules, `class`/`module` bodies, `self`, singleton classes, `defined?`. *Check:* no example in the corpus is blocked on a constant, a constant path, a class or module body, `defined?`, or a singleton method definition; `tests/eval.txt` holds the lookup order against CRuby.
- Regex engine decision and integration (engine.md, "Regex"), `Regexp` and `MatchData` basics, `=~`, `case` with regex. *Check:* `language/regexp/`. **Done:** `spinel-regex`, this workspace's own engine; 0 to 152 of 283 passing, and `scripts/regexp-oracle.rb` keeps the dialect measured against CRuby.
- `core/kernel.rb`, `core/object.rb`, minimal `Integer`, `String`, `Array`, `Hash`, `Symbol` in Ruby, enough to run the specs above. *Check:* `spinel run hello.rb` and the phase's spec directories. **Done:** `core/*.rb` loaded by `spinel-core`, a growable two-slot `Array`, `Class#allocate` per class, and `spinel run`; `language/` 545 to 619, the corpus 713 to 1,112.

**Milestone:** `language/` passes above 90%.

## Phase 2: core library

One slice per class, each driven by `core/<class>/`: `Integer`, `Float`, `String` and `Encoding` (UTF-8/US-ASCII/binary), `Symbol`, `Array`, `Hash`, `Range`, `Comparable`, `Enumerable`, `Enumerator` (needs fibers), `Proc`/`Method`/`UnboundMethod`, `Module`/`Class` reflection (`define_method`, `instance_eval`, `method_missing`, hooks), `Exception` hierarchy with `caller_locations`, `Struct`, `Data`, `Time` (primitive clock), full `Regexp`/`MatchData`, `Rational`/`Complex` (Ruby), `Math`, `GC`/`ObjectSpace` with `WeakMap`, `WeakRef`, and finalizers, `Marshal`.

- Fibers on the non-recursive interpreter plus `corosensei` for re-entrant primitives. *Check:* `core/fiber/`, `core/enumerator/`.
- String `eval`, `binding`, `instance_eval` with strings. *Check:* `core/kernel/eval_spec.rb`, `core/binding/`.

**Milestone:** `core/` passes above 80%, `mspec` itself runs on Spinel and replaces `spec/harness/`.

## Phase 3: a real runtime

- `require`/`load`/`$LOAD_PATH`/`autoload`, bytecode cache in `.spinel/bytecode/` keyed by content hash. *Check:* `core/kernel/require_spec.rb`; second run of a 500-file fixture is measurably faster.
- `IO`, `File`, `Dir`, `Process` including `fork` and `spawn`, `ENV`, signals, `Kernel#system`/backticks, `ARGV`, exit codes. *Check:* `core/{io,file,dir,process,env}/`.
- `Thread`, `Mutex`, `Queue`, `ConditionVariable`, `Monitor` on the per-Ractor lock, with the lock released around blocking calls. *Check:* `core/{thread,mutex,queue,conditionvariable}/`, `library/monitor/`.
- Vendored pure-Ruby stdlib extracted on first run: `optparse`, `erb`, `fileutils`, `time`, `net/http`, `webrick` (a gem, vendored for the phase 4 milestone). *Check:* `library/` for each. Not `set`: it is a core class as of Ruby 4.0 (`set.c`, no `lib/set.rb`), so it belongs to phase 2.
- Built-in gems whose C parts become primitives: `json` (Ruby half + Rust parser), `stringio`, `strscan`, `digest`, `zlib`, `date`, `socket`. *Check:* `library/` for each; the built-in versions match the gem versions Spinel claims.
- `ruby -e`, `-r`, `-I`, `-w`, `$0` parity so scripts and gems' shell-outs behave. *Check:* `command_line/`.
- yjit-bench subset runs; record interpreter numbers. *Check:* `bench/README.md` table, target within 1.5× of `ruby --disable-yjit`.

**Milestone:** `rake`, `minitest`, `rspec-core` run their own test suites on Spinel.

## Phase 4: tooling on the engine

The package-manager.md, cli.md, test-runner.md and build.md designs. Slices as listed in those docs, plus `crates/spinel-cli/tests/fixtures/bundler-setup` from cli.md. This phase covers pure-Ruby and built-in gems only; the `.spinel` extension path and Spinel-native gems are phase 5.

**Milestone:** `spinel add sinatra`, a Sinatra app serves requests on WEBrick, `spinel test` runs its Minitest suite in parallel, `spinel build --compile` produces a binary that runs on a clean machine, `require "bundler/setup"` works inside the app.

## Phase 5: extensions and Ractors

- `spinel-ext` public API, versioned ABI, loader, an example extension gem, `Spinel::FFI` on libffi. *Check:* a gem with a `.spinel` extension installs and loads; `library/fiddle/` subset.
- `openssl` (phased: digests, HMAC, random, then TLS for `net/https`), `psych`, `bigdecimal`, `io/console`, `etc`. *Check:* `library/` for each; `net/https` fetches a page.
- Install-time substitution and `.spinel/substitutions.json`; `pg` and `sqlite3` Spinel-native gems published under `<cpu>-<os>-spinel`. *Check:* a Sinatra + sqlite fixture; `bundle install` on the same Gemfile and lock ignores the Spinel gems and the lock is unchanged.
- `puma` and `nio4r` Spinel-native gems. *Check:* the Sinatra fixture serves on Puma.
- Ractors: creation, message passing, shareability checks, parallel execution on separate OS threads. *Check:* `core/ractor/`; a CPU-bound benchmark scales with Ractor count.

## Phase 6: JIT and GC

- Cranelift lowering of the interpreter's bytecode, call-count tiering, guards and deopt, inline caches consumed at compile time, stack maps for the GC. *Check:* full ruby/spec run with `--jit` forced on every method produces the same results; yjit-bench headline set faster than `ruby --yjit`.
- Generational, moving GC with the existing handle discipline. *Check:* same spec run; allocation-heavy benchmarks improve.

## Phase 7: Rails and reach

- Rails prerequisites: `TracePoint` (`:class`, `:call`, `:return`, `:line`, `:raise`), `Module#const_source_location`, `Class#subclasses`, Ractor-safe class-level state from the main Ractor. *Check:* `zeitwerk` and `activesupport` test suites.
- `rails new` app boots on Spinel, `spinel test` runs its suite green with SQLite, then Postgres via the native `pg`. *Check:* fixture app in CI.
- `nokogiri` Spinel-native gem on a Rust XML/HTML parser, because Rails' test helpers depend on it. *Check:* `rails-html-sanitizer` suite.
- `spinel fmt` and `spinel lint` on `spinel_ast`. `spinel ruby install` for pinned Spinel versions. Windows.

## How to vibecode this

1. One slice per session. Paste the bullet and the relevant doc section; the check is the definition of done.
2. Engine slices: run the named spec directory before and after; the PR states the delta. A slice that adds no passing specs is not done.
3. Tooling slices: the check is an integration test in `crates/spinel-cli/tests/` that shells out to the built binary. It lives inside `spinel-cli` rather than a top-level `tests/` because Cargo only guarantees a freshly rebuilt binary, via `CARGO_BIN_EXE_spinel`, to tests in the binary's own package.
4. Never mark a failing spec as expected. Skip with a reason in `spec/tags/` or fix it.
5. The core library is Ruby. If a session reaches for Rust to implement a `String` method, it must justify why Ruby cannot express it.
6. No globals. Reviews grep for `static mut`, `lazy_static`, `OnceCell` holding `Value`, and `thread_local!` holding VM objects. Anything found outside `spinel-vm/src/shared/` is a bug.
7. Numbers live in `bench/`, reproduced by script, never typed into prose.
8. When a slice reveals a doc is wrong, fix the doc in the same PR.
