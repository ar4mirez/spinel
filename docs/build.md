# Build: single executables

Goal: `spinel build --compile bin/mytool -o mytool` produces one file that runs on a machine with no Ruby, no gems, and no Spinel.

## Format

Same approach as `bun build --compile` and tebako: the output is a copy of the `spinel` binary with a payload appended.

```
[ spinel binary for target ][ payload.tar.zst ][ 32-byte sha256 of payload ][ 8-byte payload length ][ 8-byte magic "SPINEL01" ]
```

The 48-byte trailer, byte for byte:

| Offset from trailer start | Bytes | Contents |
|---|---|---|
| 0 | 32 | sha256 of the payload bytes, and nothing else |
| 32 | 8 | payload length, unsigned 64-bit **little-endian** |
| 40 | 8 | the ASCII magic `SPINEL01`; the `01` is the layout version, so a changed layout is a new magic |

The payload sits directly before the trailer, so it starts at `file length − 48 − payload length`.

On startup, `spinel` reads its own last 48 bytes (`/proc/self/exe` on Linux). Without the magic it is a plain `spinel`, and that check is one seek and one 48-byte read. If the magic is present it is a compiled app: it bounds-checks the length against the file, hashes the payload, and exits 1 with a message naming the problem if the payload is truncated (the length reaches past the start of the file) or corrupt (the hash differs from the trailer's). Then it extracts (or reuses) the payload and runs the entry point instead of parsing subcommands. The trailer and its checks are in `crates/spinel-cli/src/trailer.rs` (#98). Extracting and running the payload is #101; until then a valid compiled app says so and exits 1.

Linux: bytes appended to an ELF are ignored by the loader, so this is a plain append.

macOS (#99): appending to a signed Mach-O invalidates the signature and Gatekeeper refuses it, so the payload and its trailer are written into a `__SPINEL,__payload` section and the binary is re-signed ad hoc (`codesign --force --sign -`), which is what Bun does. The section's bytes are exactly what Linux appends, `[ payload ][ trailer ]`, so the same parser reads both; "the file" in the paragraph above is the section, and the payload's offset is from the start of the section. Every macOS `spinel` reserves the section, 16 zero bytes from a `#[used] #[link_section]` static in `crates/spinel-cli/src/macho.rs`, which the linker places as its own segment after `__DATA` and directly before `__LINKEDIT`. Writing an app grows that segment to hold the payload, rounded up to 16 KiB pages, and moves `__LINKEDIT` and every file offset that points into it (symbol table, dyld info, chained fixups, function starts, the code signature, and the rest) along by the same number of pages; no code or data moves, so nothing is relocated. At startup a Mach-O reads its header and load commands to find the section, then its last 48 bytes; a Mach-O without the section, or with bytes appended past its signature, is a plain `spinel`. Only thin 64-bit little-endian Mach-O is handled, what `cargo build` makes for one target: `spinel pack` refuses a universal, 32-bit or big-endian one, and at startup such a file is a plain `spinel`. // ponytail: `codesign` is Apple's, so an app for macOS is built on macOS; cross-building one from Linux needs an in-process ad hoc signer (a code directory of page hashes). Notarization and Developer ID signing are not done: a locally built app has no quarantine attribute, so Gatekeeper does not prompt for it, and one that is downloaded needs the user's own signing.

`spinel build --compile` (#100) assembles the payload and writes the app through this writer, from the `spinel` that runs it. The hidden `spinel pack <payload> -o <app>` stays as the debugging window onto the writer: it writes an app from any payload file and this `spinel`, appended on Linux and in the section, signed, on macOS. Both stream the payload from a file, so neither holds it in memory. `crates/spinel-cli/tests/trailer.rs` checks the Linux side and `trailer_macos.rs` the macOS one: `codesign --verify --strict` passes, the app runs and finds its payload, and the section holds the bytes Linux appends.

## The payload

```
spinel build --compile <entry> -o <app> [--include PATTERN]... [--exclude PATTERN]...
             [--group GROUP]... [--env KEY=VALUE]... [-- ARGS...]
```

The payload is `payload.tar.zst`: one tar stream, compressed with zstd, written to a file beside `<app>` and hashed as it is written, then streamed into the app (`crates/spinel-cli/src/build.rs`). Its entries, in this order:

- `manifest.json` – the entry point, relative to `app/` (`"entry": "bin/mytool"`), `spinel_version` (the version of the `spinel` the payload is packed into), `spinel_ext_abi` (`spinel_ext::ABI`, 0 until the extension API lands in phase 5), `argv` (the arguments after `--`, which the app's `ARGV` starts with) and `env` (each `--env`, an object of strings).
- `app/` – the project's files, the project being the Gemfile's directory, or the current directory when there is no Gemfile. A file is taken when `.gitignore` keeps it, as git decides it (the nearest rule wins, `.gitignore` files from deepest up, then `.git/info/exclude`, then the global excludes file; the matcher `spinel run --watch` uses), or when an `--include` matches it or a directory above it, and never when an `--exclude` does. Both take patterns as `.gitignore` writes them, relative to the project. `.git/` and `.spinel/` are never taken. The entry point must be one of the files taken, or the build stops before writing anything.
- `gem_home/` – what the project's `.spinel/gem_home` holds of the gems the app needs: the Gemfile's dependencies in the `default` group and in each `--group`, and every locked gem they reach through the lockfile, as `Bundler.setup(:default, *groups)` loads them. For each, `gems/<full name>/`, its `specifications/<full name>.gemspec`, the `bin/` stubs of its executables, and for a git gem the `bundler/gems/` checkout it lives in; the other `gem install` directories are there, empty. A gem the lockfile needs that is not installed stops the build, naming it. A path gem whose directory `app/` takes, as a `gemspec` project's own is, takes the files `app/` takes, so `.gitignore`, `--include` and `--exclude` decide it too. // ponytail: a `gemspec` project's files are then packed twice, under `app/` and `gem_home/gems/<name>-<version>/`, and each `.rb` is compiled twice; copying only the gemspec's `files` for a path gem, or pointing the path gem at `app/`, packs them once. `ruby/<ver>/` and `spinel/<ver>/`, the views Bundler reads through `BUNDLE_PATH`, are symlinks to the same directories and are not copied.
- `bytecode/` – the bytecode cache (cli.md, "Bytecode cache and require map") for every `.rb` file in `app/` and `gem_home/`, each compiled as `require` would compile it, named by the SHA-256 of its source. A file that does not parse or compile has no entry and is compiled when it is required. // ponytail: each entry is compiled under the file's path in the payload (`app/lib/greet.rb`), since the directory an app is extracted to is named by the payload's hash, which the cache is part of; running the app (#101) reads the entries rebased onto that directory.

The app being written and the directory its payload is assembled in, beside it, are never taken anywhere, so a project that writes its app inside itself, a `gemspec` project's own gem included, builds the same payload the second time as the first. Every symlink is followed, in `app/` and `gem_home/` alike, so the payload holds real copies of what they lead to; a symlink that leads nowhere or back into a directory it is in stops the build. Nothing else is in the payload: not `ext/`, the `.spinel` extensions for the target platform chosen from `.spinel/substitutions.json`, which comes with the extension loader in phase 5 (#103–#105), so a payload today has none; and not the core image or the standard library, which are already inside the `spinel` binary.

The same project builds the same payload, byte for byte: entries in a fixed order, each directory's names sorted, every mtime 0, every owner root with no names, every mode 0755 or 0644 by whether the file is executable. `crates/spinel-cli/tests/build.rs` installs `fixtures/build`'s path gems offline, builds, reads the payload back out of the app through its trailer, and checks exactly these entries: the documented directories and manifest fields, an ignored file and one an `--include` takes back, an `--exclude`, symlinks to a file and a directory copied as real files, the default group's gems and a dependency reached through one but not a `development` gem until `--group development`, one cache entry per `.rb` file and nothing else, and the same bytes from a second build.

## Runtime behavior of a compiled app

1. Read the payload hash from the trailer → `$XDG_CACHE_HOME/spinel/apps/<hash>/`. Extract once. // ponytail: extract-to-cache. An in-memory VFS with intercepted `open`/`stat` (tebako's DwarFS approach) avoids the disk write; add it if users hit read-only filesystems.
2. Set GEM_HOME/GEM_PATH to the extracted `gem_home`, load path to the app, `$0` to the entry point (the manifest's `entry`, under `app/`), `ARGV` to the manifest's `argv` then the app's own arguments, and the manifest's `env` in the environment. Use the extracted `bytecode/` as the bytecode cache, its entries rebased from their payload paths onto the extraction directory.
3. Run the entry point on the VM exactly as `spinel run` would.

This is #101; until it lands, a compiled app verifies its payload and says it cannot run yet.

Users never see a Spinel CLI: `mytool --help` is the app's `--help`.

## Cross-compiling

`--target aarch64-linux` downloads the matching `spinel` release binary into `~/.spinel/targets/` and appends a payload built for it. Pure-Ruby gems are portable. `.spinel` extensions must exist for the target platform; Spinel-native gems publish every supported platform, so this is normally a download. If one is missing the build fails with the gem name.

## Size and startup

Expected: under 30 MB binary + payload. Rails app payloads run 60–150 MB; CLIs 1–10 MB. First run pays extraction (~100 ms for a CLI); later runs are cache hits and boot as fast as `spinel run` with a warm bytecode cache.

## Not `spinel build` (yet)

Tree-shaking gem files, AOT compilation, stripping unused stdlib. Each is a measurable size win and none is needed for the feature to be useful.
