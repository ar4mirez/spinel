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

`shims/gemfile_eval.rb` defines a small `Bundler::Dsl`-shaped object (`source`, `gem`, `group`, `platforms`, `git`, `path`, `ruby`, `gemspec`, `eval_gemfile`, and the rest of Bundler 4's DSL: `platform`, `github`, `git_source`, `install_if`, `env`, `plugin`) and prints JSON. Evaluating it as Ruby means every Gemfile that Bundler accepts, Spinel accepts, including the ones with `if ENV[...]` logic. Do not try to parse Gemfiles with Prism.

`spinel run shims/gemfile_eval.rb` evaluates `BUNDLE_GEMFILE` when it is set and not empty, otherwise the nearest `gems.rb` or `Gemfile` from the current directory upward, as Bundler finds it. Each method records what `Bundler::Dsl` records and rejects what it rejects, with Bundler's message: a Symbol gem name, an unknown platform, the same gem twice with different requirements or sources, a `branch` without a git source, `git` or `path` without a block, a second global `source`, an undefined method. An error prints `There was an error parsing \`<file>\`: <message>` and `  from <file>:<line>` on stderr and exits 1; a gem listed twice alike is Bundler's warning on stderr and both entries stay. The one rule it does not copy is Bundler's rejection of an unknown `gem` option: those are kept, in `options`, for the resolver to judge. `crates/spinel-cli/tests/gemfile_eval.rs` checks every method against fixtures whose dependency lists match Bundler 4.0.20's `Bundler::Dsl`.

The JSON is pretty-printed, keys always in the order below and always present (`null` when unset); arrays keep declaration order.

```json
{
  "gemfiles": ["/abs/Gemfile", "/abs/gemfiles/common.rb"],
  "source": {"type": "rubygems", "remote": "https://rubygems.org", "cooldown": null},
  "ruby": {"versions": ["3.4.1"], "engine": "ruby", "engine_versions": ["3.4.1"], "patchlevel": null},
  "optional_groups": ["ci"],
  "gemspecs": [{"path": ".", "root": "/abs", "files": ["/abs/app.gemspec"], "name": null, "glob": null,
                "development_group": "development", "gemfile": "/abs/Gemfile"}],
  "dependencies": [
    {"name": "pg", "requirements": ["= 1.5.0.pre.rc1"], "groups": ["default"], "platforms": [],
     "source": null, "require": [], "install_if": true, "env": null, "force_ruby_platform": null,
     "gemfile": "/abs/Gemfile", "options": {}}
  ]
}
```

- `gemfiles`: every file evaluated, the Gemfile first, then each `eval_gemfile` (resolved against the directory of the file that calls it), absolute.
- `source`: the global rubygems source, or `null` when the Gemfile names none. `remote` is as written; Bundler compares remotes with a trailing `/`. `cooldown` is `source`'s `cooldown:` option.
- `ruby`: the `ruby` directive as Bundler's `RubyVersion` holds it, or `null`. `versions` are requirements with `=` dropped and `-` read as `.`; `file:` reads the version from the named file, in `.ruby-version` or `.tool-versions` form, relative to the file with the directive. `engine` defaults to `ruby`, `engine_versions` to `versions`, and `patchlevel` is `"-1"` for a prerelease Ruby.
- `optional_groups`: groups declared with `optional: true`.
- `gemspecs`: each `gemspec` directive, with the gemspec files it found in `path` (relative to `root`, the directory of the file that has the directive). They are not evaluated yet; evaluating one into its gem (from a path source) and its development dependencies (in `development_group`) is #88.
- `dependencies`: one per `gem` call that ran.
  - `requirements`: `Gem::Requirement#as_list`, so `"1.0-rc1"` is `"= 1.0.pre.rc1"` and no requirement is `[">= 0"]`.
  - `groups`: the enclosing `group` blocks then the gem's `group:`/`groups:`; `["default"]` when there are none.
  - `platforms`: the enclosing `platforms` blocks then the gem's `platform:`/`platforms:`, each one of Bundler's platform names.
  - `source`: `null` for the global source. Otherwise the gem's own `path:`, else its `git:` or git-source option (`github:`, `gitlab:`, one from `git_source`), else its `source:`, else the enclosing `source`, `git`, `github` or `path` block. One of `{"type": "rubygems", "remote", "cooldown"}`, `{"type": "git", "uri", "branch", "ref", "tag", "submodules", "glob"}`, `{"type": "path", "path", "root", "glob"}` (`path` as written, relative to `root`), or `{"type": "plugin", "plugin", "uri", "options"}` for `source "...", type: ...`.
  - `require`: `null` when not given (require the gem's name), else Bundler's autorequire list: `require: false` is `[]`.
  - `install_if`: the enclosing `install_if` conditions and the gem's `install_if:`, evaluated now, as Bundler does.
  - `env`: the enclosing `env` block's variable name (or Hash, with a Regexp written as its `inspect`), else `null`; Bundler checks it at install time, and so should Spinel.
  - `force_ruby_platform`: as given, else `null`.
  - `gemfile`: the file the `gem` call is in.
  - `options`: every other option, keys as strings.

### 2. Index client

rubygems.org's compact index, the same API Bundler uses:

- `GET /versions` – append-only list of every gem and version, ETag + `Range` for incremental updates. Cached at `~/.spinel/index/<host>/versions`.
- `GET /info/<gem>` – one line per version: `version[-platform] dep:req,dep:req|checksum:sha256,ruby:>= 3.0,rubygems:>= 3.3`.
- `GET /names`

Fetched with `reqwest`, concurrent (limit 32), ETag-validated. Private sources (`source "https://gems.example"`) use the same API; fall back to the legacy `specs.4.8.gz` only if compact index returns 404. Git and path sources are resolved by cloning and evaluating the gemspec.

The client is `crates/spinel-cli/src/compact_index.rs`, and it updates each file as Bundler 4's `CompactIndexClient::Updater` does:

- **Cache layout.** `<home>/index/<host>/` (`<home>` is `$SPINEL_HOME`, else `~/.spinel`; a source on a port other than its scheme's adds `:<port>`, and one under a path adds `-` and 16 hex digits of the path's SHA-256, so two sources never share a cache). In it: `versions`, `versions.etag`, `names`, `names.etag`, `info/<gem>`, and `info-special-characters/<gem>-<md5 of name>` for a name with anything but lowercase letters, digits, `_` and `-`, as Bundler lays them out. A gem name outside rubygems.org's `[A-Za-z0-9._-]` is refused rather than made into a path. Every file is written beside itself and renamed into place.
- **Incremental update.** With a non-empty cached copy, one request: `Range: bytes=<size - 1>-` (one byte back, so the range is never empty) and `If-None-Match` with the stored ETag. A 304 keeps the copy. A 206 is appended without its first byte and kept only when the whole result matches the SHA-256 in `Repr-Digest` (else `Digest`; as a byte sequence, a quoted string as rubygems.org sends it, or bare base64). With no digest nothing is appended, as in Bundler: appending unverified is too error prone. A 200, a server ignoring the range, replaces the copy.
- **Fallback.** No cached copy, a 206 with no digest or a mismatch (a stale or corrupt cache), or a 416 (a cache longer than the file now is): fetch the whole file, with the ETag but no range. A whole file whose digest does not match is an error and is not written. Two differences from Bundler, both toward refetching: with no cached copy no ETag is sent (Bundler's would get a 304 and keep nothing), and a response with no ETag removes the stored one rather than leaving it to vouch for a file it was not sent with.
- **`/info` skip.** An `/info/<gem>` is requested only when its cached file's MD5 differs from the checksum the cached `/versions` lists last for the gem, so a warm cache costs one request (the `/versions` 304) however many gems are asked for. A gem `/versions` does not list is not requested. That request carries no ETag, and none is stored for `/info`: the copy is already known to differ from the server's, and rubygems.org's ETag for an info file is its MD5, so a stored one still current would get a 304 that keeps the wrong copy on every run (Bundler sends it). Without one, a truncated copy is completed by the range and any other is fetched again.
- **Concurrency.** At most 32 requests in flight per client: a permit covers one file's update, which makes at most one request at a time.
- **What a 304 vouches for.** A 304 carries no digest, so a cached `/versions` or `/names` damaged after it was written, with its ETag still current, is trusted until the file changes upstream, as it is by Bundler. The cache's own writes cannot leave one: the data is renamed into place before its ETag is written.

`spinel index [--source URL] [--names] [gem...]` (hidden) runs the client and prints one line per file; `crates/spinel-cli/tests/compact_index.rs` drives it against a local server.

### 3. Resolver

`pubgrub` crate, as Bundler resolves with its vendored pubgrub-ruby. Spinel supplies:

- **Version type**: `Gem::Version` semantics ported to Rust (segments, prerelease ordering, `~>` pessimistic operator), in `crates/spinel-cli/src/gem_version.rs`, with RubyGems' own `test_gem_version.rb` table and the `~>` cases of `test_gem_requirement.rb` beside it. `spinel run`'s language-version check (cli.md) compares versions with it. `Gem::Requirement`, which reads a Gemfile's and an `/info` line's requirements, is `crates/spinel-cli/src/gem_requirement.rs`.
- **Dependency provider**: `crates/spinel-cli/src/resolver.rs` reads `/info` lines as RubyGems' `GemParser` does (`version[-platform] dep:req&req,...|checksum:...,ruby:...,rubygems:...`), filters by platform (`ruby`, `x86_64-linux`, `arm64-darwin-24`, ... using RubyGems' platform matching rules, `Gem::Platform` ported to Rust in `crates/spinel-cli/src/gem_platform.rs` with RubyGems' own `test_gem_platform.rb` beside it) and by `required_ruby_version`, and is modelled on `Bundler::Resolver` so that it chooses what Bundler 4 chooses:
  - **Candidates.** A version is up to two candidates, its `ruby` build (priority -1) and its builds for the resolution platforms (priority 1), ordered by version and then priority, so `= 1.0` covers both and `> 1.0` neither (`requirement_to_range`; `~> 1.4` ends before `2.A`). For each platform the builds `Gem::Platform` installs there are narrowed to the best match (`sort_and_filter_best_platform_match`); a platform build with the ruby build's dependencies joins the ruby candidate, and a platform candidate equal to the ruby one is dropped. A Gemfile gem needs a build for every resolution platform the dependency applies on (its `platforms:`, through Bundler's platform map), any other gem one for some platform. Builds that depend on themselves impossibly are dropped.
  - **Ruby and RubyGems.** `required_ruby_version` and `required_rubygems_version` are dependencies on two meta packages, `Ruby\0` and `RubyGems\0`, each with one version, the one resolved for (Spinel's `RUBY_VERSION` and the RubyGems it ships, unless told otherwise), which the root depends on exactly. So a Ruby requirement rules a version out the way any conflict does, and the explanation says `current Ruby version is = 2.6.10`.
  - **Prereleases** are left out of a gem's candidates unless its Gemfile requirement names one or it has a single build. A Gemfile gem with no candidate otherwise takes them, last; a resolution that fails through a gem whose prereleases were left out is retried with them, as Bundler retries (pubgrub-rs orders a derivation's causes otherwise than pubgrub-ruby, so every gem in the derivation is considered, where Bundler walks the first causes).
  - **Order.** The next gem decided is the one with at most one candidate left, then the one with the fewest candidates above its range, then the one found first, a version's dependencies being found last first, as pubgrub-ruby's propagation finds them (`Strategy#next_term_to_try_from`); its candidate is the newest, releases before prereleases unless the Gemfile asked for prereleases, a platform build before the ruby build (`GemVersionPromoter#sort_versions`).
  - **Index.** Before resolving, `/info` is brought up to date for every gem the Gemfile's gems reach through any version's dependencies, round by round, as Bundler fetches them; a gem `/versions` does not list has no builds.
- **Conflict explanations**: `crates/spinel-cli/src/resolver/report.rs` ports pubgrub-ruby's `FailureWriter` over pubgrub-rs's derivation tree, with Bundler's wording (`Could not find compatible versions`, `Gemfile depends on ...`, `... could not be found in rubygems repository ... for any resolution platforms (...)` and the matching builds after it). A dependency is stated for every version that shares it, as Bundler states it; pubgrub-rs's facts about the gaps between versions it ruled out one by one, which Bundler never needs, are folded away. One difference remains: pubgrub-rs only learns what it tried, so a range can start at the first release where Bundler's starts at a prerelease with the same dependency. A Gemfile gem no candidate satisfies fails before resolving with Bundler's `Could not find gem 'rack (>= 99)' in ...` and the builds the source has.
- **Locked preferences** (#79): when `Gemfile.lock` exists and the user did not ask to update, locked versions are the only candidates for unchanged deps (this is how `bundle install` stays stable and fast).

Not yet: lockfile shaping of `PLATFORMS` (Bundler adds `ruby` and the like to a new lockfile when every gem has a build for it, #80), a source scoped to some gems, git and path sources (#88), and `bundler` as a dependency, which Bundler pins to itself.

`spinel resolve [--platform P]... [--ruby-version V] [--rubygems-version V] <deps.json | ->` (hidden) resolves the JSON `shims/gemfile_eval.rb` prints against its source's index and prints one line per build as a lockfile's `specs:` lists them, or the explanation on stderr, exit 1. `crates/spinel-cli/tests/resolver.rs` runs it on the Gemfiles in `tests/fixtures/resolver/cases/` against `tests/fixtures/resolver/index/`, a snapshot of rubygems.org's `/info` for Sinatra, rackup, WEBrick and json and everything their versions depend on (`snapshot.sh` takes it; `index/SOURCE` dates it), served on 127.0.0.1, and checks each against what Bundler 4.0.20 resolved for the same Gemfile, snapshot, platforms and Ruby (`bundler.sh`, which records Bundler's `Gemfile.lock` or explanation): Sinatra's set, a Ruby too old for Sinatra 4, backtracking, `java` builds, prereleases asked for and not, and five failures, four of them explained word for word as Bundler explains them.

### 4. Lockfile

Parser and writer for Bundler's format: `GEM`/`PATH`/`GIT` blocks with `remote:`/`specs:`, `PLATFORMS`, `DEPENDENCIES`, `CHECKSUMS` (Bundler ≥ 2.6), `RUBY VERSION`, `BUNDLED WITH`. Round-trip test: parse every fixture lockfile and re-emit byte-identical output. Write `BUNDLED WITH` as the Bundler version Spinel emulates (pin it in one constant).

The lockfile never mentions Spinel. `PLATFORMS` lists `ruby` and the usual RubyGems platforms; Spinel-native extension substitutions (section 6) are recorded in `.spinel/substitutions.json`, not in the lock. That is what keeps the lock acceptable to Bundler.

### 5. Global store

`<home>/store/gems/<name>-<version>-<platform>-<sha256>/` holds one gem, keyed by its content: `<sha256>` is the SHA-256 of its `.gem` in hex, as the index's `/info` line (later the lockfile's `CHECKSUMS`) lists it, and `<platform>` is `ruby` for a gem with none. In it, `gem/` is `data.tar.gz` unpacked and `metadata` is the gemspec as YAML, from `metadata.gz`. A gem is fetched once for every project: one already stored with that checksum costs no request, and two sources serving different bytes under one name never share an entry. Per project, `.spinel/gem_home/gems/<full name>` is a **symlink** to the entry's `gem/`, `specifications/<full name>.gemspec` is a real file (RubyGems wants to read it), `bin/` gets generated stubs; "GEM_HOME" below has the rules.

// ponytail: symlinks. Hardlink-per-file (pnpm style) if a gem ever needs to write into its own directory; none of the top 1000 do.

The store is `crates/spinel-cli/src/gem_store.rs`:

- **Fetch.** `GET <source>/gems/<full name>.gem` through the index client's HTTP client, under its limit of 32 requests in flight, streamed to `store/tmp/` and hashed as it arrives. A `.gem` whose SHA-256 is not the listed one is refused, and a gem the index lists with no checksum is refused too, since the store is keyed by it.
- **Unpack**, as `Gem::Package` verifies and extracts one: a part named twice, a missing `metadata.gz` or `data.tar.gz`, or a part whose SHA256 or SHA512 differs from `checksums.yaml.gz` refuses the gem (SHA1, which RubyGems 2 also wrote, is not checked; those gems list SHA512 too). Signatures are not checked, as `gem install` does not check them without a trust policy. Every entry of `data.tar.gz` must name a path inside the gem's directory; directories, files (with their permission bits, never setuid, setgid or sticky) and symlinks are written, and any other kind of entry is passed over. Symlinks are made last, so nothing is written through one, and each must point inside the gem by its spelling, as RubyGems requires, and also where it physically leads once made, which catches `a/b/c/up -> ../..` followed by `a/b/c/up/x -> ../../..`. A symlink may not replace an existing directory or symlink, so no symlink checked earlier can be redirected through one made later; every symlink is checked again once all are made. A dangling one is left out with RubyGems' warning.
- **Atomicity.** The entry is built in `store/tmp/<pid>-<n>/` and renamed into `store/gems/` whole, so an entry that exists is complete; a cut connection, a refused archive or a killed process leaves only staging files, which the next run removes once the process that made them has exited. Two `spinel`s storing one gem at once both fetch it, and the second rename finds the first's entry and keeps it. Nothing is fsynced (a `ponytail:` in `gem_store.rs`).

Fetch and unpack run in parallel (`tokio`: downloads on the runtime, unpacking on blocking threads, at most one per CPU), so a cold install is network-bound, not CPU-bound: `bench/install.sh` times a cold and a warm store for the Sinatra fixture's lockfile beside `curl` fetching the same files. A warm install (everything in the store) is link creation only, plus the one index request: sub-second, and `bench/install.sh` times it too.

`spinel fetch [--source URL] <name@version[-platform]>...` (hidden) brings the index up to date, reads each gem's checksum from its `/info` line, and fetches into the store, printing one line per gem; `crates/spinel-cli/tests/gem_store.rs` drives it against a local server with real `.gem`s that `tests/fixtures/gem_store/build.sh` builds with `gem build`.

#### GEM_HOME

`crates/spinel-cli/src/gem_home.rs` links stored gems into a project's `.spinel/gem_home/`, laid out as `gem install --install-dir` lays one out:

- **Directories.** `Gem::REPOSITORY_SUBDIRECTORIES` (`build_info`, `cache`, `doc`, `extensions`, `gems`, `plugins`, `specifications`) and `bin`, as `gem install` makes them.
- **`gems/<full name>`** is a symlink to the store entry's `gem/`, the name being the gemspec's `full_name` (`name-version`, and `-platform` unless it is `ruby`). The gemspec must name the gem asked for. Each executable gains `0o111`, as `Gem::Installer#generate_bin` leaves it; the file is the store's, and the change is the same for every project.
- **`specifications/<full name>.gemspec`** is what `Gem::Installer#write_spec` writes: `installed_by_version` set to the RubyGems Spinel ships (4.0.16, in `stdlib/`), then `to_ruby_for_cache`. Loading the stored YAML needs Psych, a C extension Spinel does not have yet, and a warm install should not boot a VM per gem, so both halves are ported to Rust in `crates/spinel-cli/src/gem_spec.rs`: Psych's load for what a gemspec holds (the `Gem::` classes `Gem::SafeYAML` permits, `ScalarScanner#tokenize` for plain scalars, anchors and aliases, `!binary`), `from_yaml`'s defaults, and `to_ruby` through the same getters and `ruby_code`, `String#dump` included. Before it is written, `Gem::Installer#verify_spec`'s checks refuse what would put code into that text or a stub outside the GEM_HOME: a line break in a require path, extension or platform, a `specification_version` that is not digits, a dependency name with `<`, `>` or a line break, an executable that is not a plain file name, a `bindir` outside the gem; and only `:runtime` and `:development` load as Symbols, as `Gem::SafeYAML` permits. A gemspec whose YAML nests deeper than 256 or builds more than 1 GiB with its aliases expanded is refused where Psych would load it (a `ponytail:` in `gem_spec.rs`). Its test compares the output with CRuby running that RubyGems on fixtures written to reach the format's corners and on real gems (`tests/fixtures/gem_spec/build.sh`), byte for byte; `SPINEL_GEMSPEC_CORPUS` points the same test at a larger set made the same way.
- **`bin/<executable>`** is `Gem::Installer#app_script_text` with `--env-shebang`, whose first line on Spinel is `#!/usr/bin/env spinel` (cli.md). Spinel counts a `#!` line naming `spinel` as a Ruby one, as CRuby counts one naming its engine, so the stub runs as a program. An executable the gem does not have gets no stub, as in RubyGems; two gems with one executable is a warning, and the later wins.
- **Not done.** `cache/` stays empty: the store does not keep the `.gem` once unpacked, and nothing `Bundler.setup` or `bundle exec` reads needs it. Extensions are not built (#83 substitutes the built-in gems, phase 5 the native ones): a gem that has some is linked with a warning, and RubyGems will not activate it. `plugins/` gets no `rubygems_plugin.rb` stubs.
- **Idempotent.** What is already right is not written again, so linking the same gems twice writes nothing. A gem a previous link made and the list no longer names is removed: its symlink (only a symlink into the store counts as one a link made), its gemspec, and its bin stubs while they are still the ones a link writes. Nothing else in the GEM_HOME is touched, so a gem `gem install`ed into it stays. Every gemspec is read before anything is written, so a gem RubyGems would refuse leaves the GEM_HOME as it was; each file and symlink is made beside its place and renamed into it.

`spinel fetch --gem-home DIR ...` links what it fetched, only when every gem was fetched, and prints `gem home: <dir>` and then `<full name>: linked` (something written), `current` (nothing to write) or `removed`. `crates/spinel-cli/tests/gem_home.rs` drives it against a local server with the store's fixtures and checks the GEM_HOME against the one RubyGems makes of the same `.gem`s (`tests/fixtures/gem_home/build.sh`), then reads it on Spinel through RubyGems (`require`, `Gem::Specification`, a bin stub run by its `#!`), `Bundler.setup` and `bundle exec`. Bundler needs its own `exe/bundle`, which Spinel's stdlib does not carry yet (#71), so that test puts one where Bundler looks first; nothing of the GEM_HOME is adjusted for it.

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
5. `bench/install.sh`: cold and warm install of the Rails fixture, Spinel vs Bundler. Until `spinel install` lands, it times the store alone, cold and warm, for the Sinatra fixture's lockfile, against `curl`.
