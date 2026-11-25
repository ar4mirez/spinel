# Spinel

**A Ruby engine and toolchain built from scratch in Rust, shipped as one binary.**

Spinel runs Ruby programs on its own virtual machine and does the work you would otherwise use several tools for: it installs gems from a Bundler-compatible `Gemfile.lock`, runs Minitest and RSpec suites in parallel, runs a gem's executable without installing it, and packs an app into a single executable. No CRuby, mruby or other Ruby VM code is linked, vendored or called. The parser is Prism, and most of the core library is written in Ruby. The target is full compatibility with Ruby 4.0, measured against [ruby/spec](https://github.com/ruby/spec). Spinel is for Ruby developers who want to try a new engine on their scripts, tools and small web apps, and for anyone who wants to help build one.

## Status

**Early and experimental. v0.1.0 is the first release.** It is not ready for production use.

- **What works.** Ordinary Ruby programs run, and most of ruby/spec passes. The pass counts for each directory are in [`bench/spec-status.md`](bench/spec-status.md), which CI regenerates; nothing is marked as an expected failure. Sinatra apps install from rubygems.org and serve requests.
- **Performance.** There is no JIT yet, so Spinel is an interpreter, and it is slower than CRuby's own interpreter on most of the benchmarks measured. See [`bench/README.md`](bench/README.md) for the numbers.
- **Not yet:** Rails ([#130](https://github.com/ar4mirez/spinel/issues/130), [#131](https://github.com/ar4mirez/spinel/issues/131)), the Cranelift JIT ([#120](https://github.com/ar4mirez/spinel/issues/120)–[#125](https://github.com/ar4mirez/spinel/issues/125)), a generational moving GC ([#126](https://github.com/ar4mirez/spinel/issues/126)), CRuby C extensions (never: see [Compatibility](#compatibility)), Windows ([#139](https://github.com/ar4mirez/spinel/issues/139)), `spinel fmt` and `spinel lint` ([#136](https://github.com/ar4mirez/spinel/issues/136), [#137](https://github.com/ar4mirez/spinel/issues/137)).

Planned work is in [`docs/roadmap.md`](docs/roadmap.md) and the [milestones](https://github.com/ar4mirez/spinel/milestones).

## Install

Each [GitHub release](https://github.com/ar4mirez/spinel/releases) has a prebuilt binary for four platforms: `spinel-x86_64-linux`, `spinel-aarch64-linux`, `spinel-x86_64-macos` and `spinel-aarch64-macos`.

### With mise

```sh
mise use -g github:ar4mirez/spinel          # latest release
mise use -g github:ar4mirez/spinel@0.1.0    # a pinned version
spinel --version                            # spinel 0.1.0 (ruby 4.0.0) [x86_64-linux]
```

mise does not install a release younger than its `minimum_release_age` setting (24 hours by default: mise 2026.11.12 applies it with the setting unset), so for the first day after a release the first line gets the release before it, or, while there is none, fails with `no versions found ... matching minimum_release_age`. Name the version instead, as in the second line. A project pins its own copy in its `mise.toml` with `mise use github:ar4mirez/spinel@0.1.0` (no `-g`).

### Download a binary

Each release asset is the bare executable, with no archive around it, at `https://github.com/ar4mirez/spinel/releases/download/v<version>/spinel-<triple>`:

```sh
curl -fsSL -o spinel https://github.com/ar4mirez/spinel/releases/download/v0.1.0/spinel-x86_64-linux
chmod +x spinel
sudo mv spinel /usr/local/bin/    # or any directory on your PATH
```

The binary links the system's OpenSSL 3, so it needs:

- **Linux:** glibc 2.39 or newer (the binaries are built on Ubuntu 24.04) and the OpenSSL 3 shared libraries (`libssl.so.3`, for example from the `libssl3` package).
- **macOS:** Homebrew's `openssl@3` (`brew install openssl@3`). The binary loads it from Homebrew's default prefix: `/opt/homebrew` on Apple silicon, `/usr/local` on Intel. The binaries are signed ad hoc, not notarized. A file downloaded with `curl` runs as it is. If you downloaded it with a browser, remove the quarantine flag first: `xattr -d com.apple.quarantine spinel`.

To build from source instead, see [Building from source](#building-from-source).

## Quick start

```sh
mkdir hello && cd hello
spinel init                       # Gemfile, .spinel-version, test/, .gitignore
spinel add sinatra rackup webrick # resolve, install, write Gemfile.lock

cat > app.rb <<'RUBY'
require "sinatra/base"

class App < Sinatra::Base
  set :server, "webrick"
  get("/") { "Hello from #{RUBY_ENGINE} #{RUBY_ENGINE_VERSION}" }
end

App.run!
RUBY
spinel run app.rb                 # serves on http://localhost:4567
```

Tests, tools and packaging:

```sh
spinel add minitest
mkdir -p test && cat > test/hello_test.rb <<'RUBY'
require "minitest/autorun"
class HelloTest < Minitest::Test
  def test_engine = assert_equal "spinel", RUBY_ENGINE
end
RUBY
spinel test                       # parallel workers; --watch, -j N, FILE:LINE

spinel x rake --version           # run a gem's executable without adding it to the project
spinel build --compile app.rb -o hello   # one self-contained executable
./hello
spinel -e 'puts RUBY_VERSION'     # ruby's own command-line flags work too
```

`spinel install` installs from an existing `Gemfile.lock`, and `--frozen` refuses to change it, as Bundler's frozen mode does. Run `spinel help` to see every command.

A gem's executables are installed in `.spinel/gem_home/bin/`. Run one in the project with `spinel run .spinel/gem_home/bin/<name>` (for example `spinel run .spinel/gem_home/bin/puma`). `spinel x` runs a gem outside the project instead.

[spinel-demo](https://github.com/ar4mirez/spinel-demo) is an example Spinel project: this quick start, a command-line tool and a Rack app on Puma, with run steps and a smoke test.

## What works today

Each item below is covered by tests in this repository: ruby/spec under `spec/`, and integration tests in `crates/spinel-cli/tests/` that run the built `spinel` binary.

| Area | What is there |
|---|---|
| Engine | A bytecode VM with inline caches and shapes for instance variables, and a precise mark-sweep GC with one heap per Ractor. It has exceptions, fibers, threads on OS threads, and Ractors that run in parallel. The core classes are written in Ruby (`core/*.rb`) over Rust primitives. |
| Standard library | Ruby 4.0's pure-Ruby standard library is vendored. The libraries whose CRuby versions are written in C ship built in: `json`, `openssl` (digests, HMAC, keys, certificates, TLS, so `net/https` works; `Cipher` and `KDF` are not there yet), `psych`/`yaml`, `bigdecimal`, `zlib`, `socket`, `date`, `digest`, `etc`, `io/console`, `fiddle` and more. |
| Gems | `spinel install`, `add`, `remove` and `update` read and write `Gemfile.lock` as Bundler 4 does, keep a global content-addressed gem store, and set gems up in a standard `GEM_HOME` (`.spinel/gem_home/`), so `require "bundler/setup"` and CRuby's `bundle exec` work in the same project. |
| `spinel test` | Minitest and RSpec suites, run on forked parallel workers with the slowest files handed out first. It can re-run a single `FILE:LINE`, re-run tests on file changes with `--watch`, and report as `json` or `junit` for CI. |
| `spinel x` | Runs a gem's executable from a cached environment of its own, like `gem exec` or `uvx`. Gems that need a C extension do not work here yet. |
| `spinel build` | `--compile` packs the project, its gems and a warmed bytecode cache into one executable. `--target <triple>` builds for another of the four release platforms ([docs/build.md](docs/build.md)). |
| Native code | `spinel-ext`, a Rust API with a versioned ABI for extensions loaded by `require`, and `Spinel::FFI` on libffi, which `fiddle` is built on. |

## Compatibility

- **Ruby version.** Spinel implements one language version at a time, currently Ruby 4.0: `RUBY_VERSION` is `"4.0.0"` and `RUBY_ENGINE` is `"spinel"`. Behavior is checked against ruby/spec first, then against CRuby 4.0.9. A `.ruby-version` or Gemfile `ruby` line that asks for a later Ruby, or a `.spinel-version` that asks for a later Spinel, only prints a warning, unless you pass `spinel run --strict-ruby-version` ([docs/cli.md](docs/cli.md)).
- **Platforms.** Linux x86_64 and aarch64, macOS x86_64 and arm64. CI builds Spinel and runs `cargo test` on Linux x86_64 and macOS arm64, and generates the ruby/spec table on Linux. There is no Windows support.
- **C extensions.** CRuby C extensions do not load, and Spinel has no CRuby C API. `spinel install` does not compile gems' C extensions. The default gems that are C in CRuby are built in (see above). Other gems with C extensions need a Spinel-native replacement: `spinel install` fails, naming the gem, on one that has none.
- **Sinatra** works. It installs from rubygems.org and serves on WEBrick (`crates/spinel-cli/tests/install.rs`).
- **Puma** runs through `puma-spinel` (with `nio4r-spinel`). On Linux, Puma's own test suite gets the same result as on CRuby 4.0.9, test for test, apart from tests that neither runtime can run (`tests/puma.rs`). Like the other Spinel-native gems in [`gems/`](gems/) (`sqlite3-spinel`, which a Sinatra app reads and writes SQLite through in `tests/sqlite3.rs`, and `pg-spinel`), it is **not published to rubygems.org yet**. Until it is, you build both gems yourself and serve them from a gem source of your own ([docs/package-manager.md](docs/package-manager.md), section 6):
  1. Clone Spinel at the tag of the `spinel` you run (`git clone --branch v0.1.0 https://github.com/ar4mirez/spinel`). Building needs what [Building from source](#building-from-source) needs.
  2. In that clone, build both gems: `spinel run gems/nio4r-spinel/build.rb --out DIR` and `spinel run gems/puma-spinel/build.rb --out DIR`. They write `nio4r-spinel-2.7.5.1-<platform>.gem` and `puma-spinel-8.1.2.1-<platform>.gem`.
  3. Serve a compact index, the format rubygems.org serves, with rubygems.org's `/info` lines for `puma` 8.1.2 and `nio4r` 2.7.5 (the versions the substitutes stand in for), lines for the two substitutes, and their `.gem` files. Spinel reads a substitute only from the source the locked gem comes from, so pin both gems to that source in the Gemfile: `source "http://127.0.0.1:9293" do gem "puma", "8.1.2"; gem "nio4r", "2.7.5" end`.
  4. Install with `SPINEL_SUBSTITUTES=puma,nio4r spinel install`. `.spinel/substitutions.json` lists what was substituted, and the lockfile still names upstream's `puma` and `nio4r`. The gem source is needed whenever `spinel install` runs, not when the app does.
  5. Start Puma with `spinel run .spinel/gem_home/bin/puma`.

  [spinel-demo](https://github.com/ar4mirez/spinel-demo) is a worked example of these steps.

  Until [#894](https://github.com/ar4mirez/spinel/issues/894) is fixed, `spinel install --frozen` rejects a lockfile whose gems come from a `source` block.
- **Rails** does not boot yet. `spinel install` can install a `rails new` lockfile, but loading Rails waits on the `zeitwerk` and `activesupport` test suites passing ([#130](https://github.com/ar4mirez/spinel/issues/130)) and on a first boot ([#131](https://github.com/ar4mirez/spinel/issues/131)).

## Building from source

You need Rust 1.85 or newer, a C compiler, `pkg-config`, and OpenSSL 3 with its headers. The build refuses older OpenSSL versions and LibreSSL.

```sh
# Debian/Ubuntu: sudo apt-get install build-essential pkg-config libssl-dev
# macOS:         brew install openssl@3 && export OPENSSL_DIR="$(brew --prefix openssl@3)"

git clone https://github.com/ar4mirez/spinel && cd spinel
cargo build --release          # target/release/spinel
cargo test                     # Rust unit tests and the CLI integration tests

git submodule update --init    # ruby/spec and mspec, needed only for the spec runs
scripts/spec.sh core/array     # run one ruby/spec directory on Spinel
scripts/bench.sh               # benchmarks; see bench/README.md
```

## Documentation

- [Design](docs/design.md): the settled decisions and why the engine is built from scratch
- [Architecture](docs/architecture.md): crates, binary layout and dependencies
- [Engine](docs/engine.md): values, heap and GC, bytecode, the interpreter, the core library, Ractors, extensions
- [CLI and runtime](docs/cli.md): `spinel run`, how Spinel identifies itself, the bytecode cache, Bundler interop
- [Package manager](docs/package-manager.md): Gemfile and lockfile compatibility, resolver, store, substitutes
- [Test runner](docs/test-runner.md) and [Build](docs/build.md): `spinel test` and `spinel build`
- [Roadmap](docs/roadmap.md): the phases, as slices that can each be checked
- [Changelog](CHANGELOG.md)

## Contributing

Issues and pull requests are welcome. Work is planned as small slices in [`docs/roadmap.md`](docs/roadmap.md), each tracked as an issue under a phase [milestone](https://github.com/ar4mirez/spinel/milestones). Each slice gets its own pull request and its own check.

- **Engine changes** are done when they make more of ruby/spec pass: name the spec files or directories that newly pass. Never mark a failing spec as expected. Either fix it, or skip it with a reason in `spec/tags/`, in the format described in [`spec/tags/README.md`](spec/tags/README.md).
- **Tooling changes** are done when there is an integration test in `crates/spinel-cli/tests/` that runs the built `spinel` against a fixture.
- If a method can be written in Ruby, it goes in `core/*.rb`. Rust is only for what Ruby cannot express.
- The settled decisions and working rules are in [`CLAUDE.md`](CLAUDE.md): no CRuby code, no global mutable VM state, and compatibility with Bundler's file formats. Please don't reopen those decisions in a pull request.

## License

Spinel is released under the [MIT License](LICENSE), Copyright (c) 2026 Angel Ramirez. Vendored code keeps its own license: the Ruby standard library under [`stdlib/LICENSE/`](stdlib/LICENSE/) (Ruby's license or the 2-clause BSD license, with per-library files), and the upstream gems in `gems/*-spinel/` under the `LICENSE-*` file in each directory.
