# Package manager

Goal: `spinel install` on a real `Gemfile.lock` finishes in seconds, produces a GEM_HOME that `bundle exec` would also accept, and never asks the user for a C compiler.

The package manager is Rust in `spinel-cli`. The only Ruby it runs (Gemfile and gemspec evaluation, bin stubs) runs on Spinel's VM, which is why it lands in phase 4.

## Compatibility contract

| Artifact | Spinel behavior |
|---|---|
| `Gemfile` | Read. Evaluated as Ruby on Spinel's VM, never parsed by hand. |
| `Gemfile.lock` | Read and written. Bundler must accept Spinel's output and vice versa. |
| `.ruby-version` | Read. |
| `GEM_HOME` layout (`specifications/`, `gems/`, `extensions/`, `bin/`) | Written to `.spinel/gem_home/`. Standard, so `Gem::Specification`, `Bundler.setup`, and every gem-aware library work unchanged. |
| `~/.gem`, `~/.bundle/config` | Ignored. `BUNDLE_GEMFILE` env var is honored. Mirrors (`bundle config mirror`) in phase 5. |
| `ruby "x.y"` directive | Read, warns on mismatch. See cli.md, "Language version". |

## Pipeline

```
Gemfile ──(ruby: gemfile_eval.rb)──▶ deps JSON ──▶ resolver ◀── compact index (cached)
                                                       │
                                     Gemfile.lock ◀────┴────▶ install plan
                                                                  │
                                    global store ◀── fetch .gem ──┤
                                                                  ▼
                                                        .spinel/gem_home/ (links) + ext download + bytecode warm + require map
```

### 1. Gemfile evaluation

`shims/gemfile_eval.rb` defines a tiny `Bundler::Dsl`-shaped object (`source`, `gem`, `group`, `platforms`, `git`, `path`, `ruby`, `gemspec`, `eval_gemfile`) and prints JSON. Evaluating it as Ruby means every Gemfile that Bundler accepts, Spinel accepts, including the ones with `if ENV[...]` logic. ~80 lines. Do not try to parse Gemfiles with Prism.

### 2. Index client

rubygems.org's compact index, the same API Bundler uses:

- `GET /versions` – append-only list of every gem and version, ETag + `Range` for incremental updates. Cached at `~/.spinel/index/<host>/versions`.
- `GET /info/<gem>` – one line per version: `version[-platform] dep:req,dep:req|checksum:sha256,ruby:>= 3.0,rubygems:>= 3.3`.
- `GET /names`

Fetched with `reqwest`, concurrent (limit 32), ETag-validated. Private sources (`source "https://gems.example"`) use the same API; fall back to the legacy `specs.4.8.gz` only if compact index returns 404. Git and path sources are resolved by cloning and evaluating the gemspec.

### 3. Resolver

`pubgrub` crate. Spinel supplies:

- **Version type**: `Gem::Version` semantics ported to Rust (segments, prerelease ordering, `~>` pessimistic operator), in `crates/spinel-cli/src/gem_version.rs`, with RubyGems' own `test_gem_version.rb` table and the `~>` cases of `test_gem_requirement.rb` beside it. `spinel run`'s language-version check (cli.md) compares versions with it until the resolver does.
- **Dependency provider**: reads `/info` lines, filters by platform (`ruby`, `x86_64-linux`, `arm64-darwin-24`, ... using RubyGems' platform matching rules, `Gem::Platform` ported to Rust in `crates/spinel-cli/src/gem_platform.rs` with RubyGems' own `test_gem_platform.rb` beside it) and by `required_ruby_version`.
- **Locked preferences**: when `Gemfile.lock` exists and the user did not ask to update, locked versions are the only candidates for unchanged deps (this is how `bundle install` stays stable and fast).

pubgrub gives readable conflict explanations for free, which is a visible upgrade over Bundler's.

### 4. Lockfile

Parser and writer for Bundler's format: `GEM`/`PATH`/`GIT` blocks with `remote:`/`specs:`, `PLATFORMS`, `DEPENDENCIES`, `CHECKSUMS` (Bundler ≥ 2.6), `RUBY VERSION`, `BUNDLED WITH`. Round-trip test: parse every fixture lockfile and re-emit byte-identical output. Write `BUNDLED WITH` as the Bundler version Spinel emulates (pin it in one constant).

The lockfile never mentions Spinel. `PLATFORMS` lists `ruby` and the usual RubyGems platforms; Spinel-native extension substitutions (section 6) are recorded in `.spinel/substitutions.json`, not in the lock. That is what keeps the lock acceptable to Bundler.

### 5. Global store

`~/.spinel/store/gems/<name>-<version>-<platform>/` holds the unpacked `.gem` (verified against the index's sha256). Per project, `.spinel/gem_home/gems/<name>-<version>` is a **symlink** into the store, `specifications/*.gemspec` is a real file (RubyGems wants to read it), `bin/` gets generated stubs.

// ponytail: symlinks. Hardlink-per-file (pnpm style) if a gem ever needs to write into its own directory; none of the top 1000 do.

Fetch and unpack run in parallel (`tokio`), so a cold Rails install is network-bound, not CPU-bound. A warm install (everything in the store) is link creation only: sub-second.

### 6. Native extensions

There is no `extconf.rb`, no mkmf, and no C compiler in the loop. See engine.md, "Native extensions and FFI". The resolver works on the ordinary gem set, exactly as Bundler would lock it; substitution happens at install time and is invisible to the lockfile.

Per locked gem, in order:

1. **Built-in gem.** Spinel ships pinned versions of the default gems whose C parts are Spinel primitives: `json`, `psych`, `date`, `bigdecimal`, `digest`, `openssl`, `zlib`, `etc`, `io-console`, `stringio`, `strscan`, `racc`, `monitor`, and others listed in engine.md. If the lock pins one of these, Spinel uses its built-in copy and skips the gem's `ext/`. A different patch version is silent; a different minor version warns; a different major version fails, because the gem's Ruby half may expect a different primitive surface.
2. **Spinel-native gem.** For gems like `pg`, `sqlite3`, `nio4r` and `puma`, a substitute is published to rubygems.org under its own name, `<gem>-spinel` (`pg-spinel`), with ordinary RubyGems platforms. These gems contain a `.spinel` extension built with `spinel-ext`, or pure Ruby on `Spinel::FFI`, and present the upstream gem's Ruby API (`require "pg"`). On any other Ruby, requiring one raises a `LoadError` that names Spinel. The rules are in `crates/spinel-cli/src/gem_substitute.rs`:
   - **Which gems.** Only gems on Spinel's built-in list of substitutes (the "Spinel-native gems list" of step 4) are substituted. A gem whose name happens to end in `-spinel` is never trusted: anyone can push one, and rubygems.org already has gems such as `bundler-spinel` that are not Spinel's. A name joins the list once the project owns it on rubygems.org.
   - **Version.** A substitute's version is the locked upstream version string, a `.`, and a revision, which is a positive integer with no leading zero. `pg 1.5.0` is substituted by `pg-spinel 1.5.0.1`, `1.5.0.2`, and so on, and the highest revision wins. The match is on the lock's string, as RubyGems normalizes it (`2.0.0-rc1` is `2.0.0.pre.rc1`), so `pg-spinel 1.5.0.1` substitutes only `pg 1.5.0`, never `1.5` and never `1.5.0.1`. No `Gem::Requirement` expresses this, so the rule is a function. A substitute never moves the locked version: if no substitute exists for exactly that version, the gem falls through to step 4.
   - **Platform.** Builds use the standard platform for each system Spinel runs on, and Linux names its libc, because a `.spinel` extension links one: `x86_64-linux-gnu`, `x86_64-linux-musl`, `aarch64-linux-gnu`, `aarch64-linux-musl`, `arm64-darwin`, `x86_64-darwin`, or `ruby` for a pure-Ruby substitute. Each of Spinel's `RUBY_PLATFORM`s accepts exactly one of them. A bare `x86_64-linux` would also be accepted on musl. Among the builds of the newest revision that `Gem.platforms` accepts, Spinel takes the one `sort_best_platform_match` ranks first, as RubyGems would.
   - **Install.** The GEM_HOME gets the locked identity: `specifications/pg-1.5.0.gemspec`, with the locked name, version and platform and upstream's runtime dependencies, and `gems/pg-1.5.0` pointing at the substitute's files. `Bundler.setup` running on Spinel therefore finds the `pg 1.5.0` the lock names. A substitute that needs a runtime dependency upstream does not have fails the install, because the lock would not contain it. `.spinel/substitutions.json` records `pg 1.5.0 ruby → pg-spinel 1.5.0.1 arm64-darwin`, and the lock keeps `pg (1.5.0)`.
   - **Stock Bundler** resolves by name: it reads `/info/pg` and never `/info/pg-spinel`, so it never installs a substitute in place of the gem. This was verified against RubyGems / Bundler 4.0.20 for `bundle lock` on ten platforms, `bundle install`, `bundle update`, `gem install` on four platforms, and a requirement only `pg-spinel` satisfies; each one left `pg-spinel` out. A Gemfile that names `pg-spinel` itself gets it as an ordinary gem with no substitution, and Spinel warns.

   Spinel-native gems were first planned under a platform, `<cpu>-<os>-spinel`, which Bundler would have to treat as non-matching. RubyGems reads most of those spellings as plain platforms: `arm64-darwin-spinel` is `arm64-darwin`, `gem build` names it `pg-1.5.0-arm64-darwin.gem`, and 20 of the 78 spellings tried match a stock platform (#76). So that scheme was dropped, and the requirement that the platform be non-matching no longer applies. The table in `crates/spinel-cli/src/gem_platform/tests.rs` stays as the record.
3. **Pure-Ruby gem**, the common case. Nothing to build.
4. **Gem with a CRuby C extension and no substitute.** Installation succeeds if the gem also ships a pure-Ruby fallback (`racc` does, `nokogiri` does not). Otherwise `spinel install` fails with the gem name and a link to the Spinel-native gems list. // ponytail: fail loudly; a CRuby C-API compatibility layer is a non-goal.

The `spinel-ext` ABI version is part of the ext cache key `~/.spinel/store/ext/<name>-<ver>-<platform>-<abi>/`, so upgrading Spinel re-downloads matching extensions and builds nothing.

### 7. Post-install

- Warm the bytecode cache for every new file (in-process, parallel).
- Regenerate `.spinel/require_map.bin` (cli.md).
- Write `.spinel/substitutions.json` and `.bundle/config` with `BUNDLE_PATH` (cli.md, "Bundler running on Spinel").
- Write `Gemfile.lock` if it changed.
- Print what changed, uv-style: `+ rails 8.1.0`, `- foo 1.2`.

## Commands

| command | does |
|---|---|
| `spinel install [--frozen]` | resolve (respecting lock) + install. `--frozen` errors if lock would change (CI). |
| `spinel add <gem>[@req] [--group dev] [--git url]` | edit Gemfile (append a `gem` line using `spinel_ast` to find the insertion point), then install |
| `spinel remove <gem>` | inverse |
| `spinel update [gem...]` | unlock named gems (or all) and resolve |
| `spinel x <gem> [args]` | see cli.md |
| `spinel outdated` | phase 5 |

`bundle exec foo` has no equivalent because `spinel run foo` and `spinel x` already set up the environment; `.spinel/gem_home/bin` is on `PATH` inside `spinel run`.

## Validation targets

1. `crates/spinel-cli/tests/fixtures/plain`: `gem "rake"`; install, `spinel run -e 'require "rake"'`.
2. `crates/spinel-cli/tests/fixtures/native-gem`: a gem shipping a `.spinel` extension; install and load it (phase 5).
   `crates/spinel-cli/tests/fixtures/builtin-gem`: a lock pinning `json` and `bigdecimal`; install uses the built-ins and skips `ext/`.
3. `crates/spinel-cli/tests/fixtures/sinatra-lock`: a real Sinatra app lockfile; install, serve a request. Run the same `Gemfile.lock` through `bundle install` and diff.
4. `crates/spinel-cli/tests/fixtures/rails-lock` (phase 7): a `rails new` lockfile; install, `spinel run bin/rails runner 'puts Rails.version'`.
5. `bench/install.sh`: cold and warm install of the Rails fixture, Spinel vs Bundler.
