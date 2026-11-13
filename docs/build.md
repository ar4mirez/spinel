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

Linux: bytes appended to an ELF are ignored by the loader, so this is a plain append. macOS: appending to a signed Mach-O invalidates the signature and Gatekeeper refuses it, so the payload is written into a `__SPINEL,__payload` segment and the binary is re-signed ad hoc (`codesign -s -`), which is what Bun does. The trailer layout is the same in both cases; only where it sits differs.

Payload contents:

- `app/` – the project files (respecting `.gitignore` plus `--include`/`--exclude`)
- `gem_home/` – the project's `.spinel/gem_home` with symlinks resolved into real copies, only gems in the lock's default + requested groups
- `ext/` – `.spinel` extensions for the *target* platform, chosen from `.spinel/substitutions.json`
- `bytecode/` – warmed bytecode cache so first run is fast
- `manifest.json` – entry point, Spinel version, `spinel-ext` ABI, argv defaults, env

The core image and stdlib are already inside the `spinel` binary, so they are not duplicated.

## Runtime behavior of a compiled app

1. Read the payload hash from the trailer → `$XDG_CACHE_HOME/spinel/apps/<hash>/`. Extract once. // ponytail: extract-to-cache. An in-memory VFS with intercepted `open`/`stat` (tebako's DwarFS approach) avoids the disk write; add it if users hit read-only filesystems.
2. Set GEM_HOME/GEM_PATH to the extracted `gem_home`, load path to the app, `$0` to the entry point.
3. Run the entry point on the VM exactly as `spinel run` would.

Users never see a Spinel CLI: `mytool --help` is the app's `--help`.

## Cross-compiling

`--target aarch64-linux` downloads the matching `spinel` release binary into `~/.spinel/targets/` and appends a payload built for it. Pure-Ruby gems are portable. `.spinel` extensions must exist for the target platform; Spinel-native gems publish every supported platform, so this is normally a download. If one is missing the build fails with the gem name.

## Size and startup

Expected: under 30 MB binary + payload. Rails app payloads run 60–150 MB; CLIs 1–10 MB. First run pays extraction (~100 ms for a CLI); later runs are cache hits and boot as fast as `spinel run` with a warm bytecode cache.

## Not `spinel build` (yet)

Tree-shaking gem files, AOT compilation, stripping unused stdlib. Each is a measurable size win and none is needed for the feature to be useful.
