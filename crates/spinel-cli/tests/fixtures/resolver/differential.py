#!/usr/bin/env python3
"""Compare `spinel resolve` with Bundler on random small indexes (#78).

An opt-in tool, never run by CI. Each case is a few gems with a few versions,
prereleases among them, depending on one another with requirements that name
prereleases and releases, and a Gemfile asking for one or two of them. Every
index is served on 127.0.0.1; Bundler resolves it through `bundler_oracle.rb`
(Ruby 4.0.0, RubyGems 4.0.16, x86_64-linux, as Spinel resolves it), and the
built `spinel resolve` resolves the same dependencies. Each case lands in one
of:

  same        both resolved to the same gems
  both-failed both failed (explanations are not compared)
  spinel-miss Spinel failed where Bundler resolved
  differ      both resolved, to different gems
  spinel-over Spinel resolved where Bundler failed

and a case of the last three kinds is kept under --out with both results.

  crates/spinel-cli/tests/fixtures/resolver/differential.py \\
      --spinel target/debug/spinel --cases 300 --seed 1 --out /path/to/scratch

With --locked (#79), each case is locked by Bundler, its Gemfile then edited
(left as it is, a gem added or taken out, or a requirement changed), and the
edit locked again by Bundler with the first lockfile in place and by `spinel
resolve --locked` with Bundler's first lockfile; the whole lockfiles are
compared, byte for byte. A case whose first lock fails is `no-lock`.
--steps N makes it N edits in a row, each locked with the lockfile Bundler
wrote for the one before, so a later edit meets versions older than the
newest; the case is the first step whose lockfiles differ. A locked case
that differs is resolved again with no lockfile by both, and its kind gets
`-unlocked-too` when that differs as well: a difference of the resolver
alone (#816), not of the lockfile's hold.

With --locked --updates (#87), an edit may also be an update with the Gemfile
left as it is: `bundle lock --update` of one gem the lockfile locks, or of
every gem, against `spinel resolve --locked --update[=GEM]`.

Written with Bundler 4.0.20 on Ruby 4.0.9.
"""

import argparse
import concurrent.futures
import hashlib
import http.server
import json
import os
import random
import shutil
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))

VERSIONS = ["0.9", "1.0", "1.1", "2.0.a", "2.0.beta", "2.0", "2.1.rc1", "3.0.pre", "3.0"]
REQUIREMENTS = [
    [">= 0"], [">= 1.0"], ["< 2"], ["~> 1.0"], [">= 2.0.a"], [">= 2.0"], ["< 3"],
    [">= 3"], ["= 2.0.beta"], ["~> 2.0.a"], [">= 1.0", "< 3"], ["> 2.0"], ["< 2.0.beta"],
]


def random_case(rng):
    """The /info lines of each gem and the Gemfile's dependencies."""
    names = [f"g{i}" for i in range(rng.randint(2, 5))]
    info = {}
    for i, name in enumerate(names):
        versions = sorted(rng.sample(VERSIONS, rng.randint(1, 4)), key=VERSIONS.index)
        lines = []
        for version in versions:
            deps = []
            for other in names[i + 1:]:
                if rng.random() < 0.45:
                    deps.append(f"{other}:" + "&".join(rng.choice(REQUIREMENTS)))
            lines.append(f"{version} {','.join(deps)}".rstrip())
        rng.shuffle(lines)  # /info lists versions in the order they were pushed
        info[name] = "".join(f"{line}\n" for line in lines)
    gemfile = [(names[0], rng.choice(REQUIREMENTS) if rng.random() < 0.3 else [])]
    if len(names) > 1 and rng.random() < 0.5:
        other = rng.choice(names[1:])
        gemfile.append((other, rng.choice(REQUIREMENTS) if rng.random() < 0.3 else []))
    return info, gemfile


def edit_gemfile(rng, info, gemfile, updates=False):
    """The Gemfile after one edit: as it was, a gem added or taken out, or
    one requirement changed; with `updates`, also an update of every gem
    (`update`) or of the lockfile's k-th gem by name (`update:k`), the
    Gemfile as it was."""
    names = sorted(info)
    edited = [list(dep) for dep in gemfile]
    kinds = ["same", "add", "drop", "change", "change"]
    if updates:
        kinds += ["update", "update-one", "update-one"]
    kind = rng.choice(kinds)
    if kind == "update-one":
        return f"update:{rng.randrange(1000)}", [tuple(dep) for dep in edited]
    if kind == "update":
        return kind, [tuple(dep) for dep in edited]
    absent = [n for n in names if n not in [d[0] for d in edited]]
    if kind == "add" and absent:
        edited.append([rng.choice(absent), rng.choice(REQUIREMENTS) if rng.random() < 0.4 else []])
    elif kind == "drop" and len(edited) > 1:
        edited.pop(rng.randrange(len(edited)))
    elif kind == "change":
        i = rng.randrange(len(edited))
        edited[i][1] = rng.choice(REQUIREMENTS) if rng.random() < 0.7 else []
    else:
        kind = "same"
    return kind, [tuple(dep) for dep in edited]


def write_index(directory, info):
    os.makedirs(os.path.join(directory, "info"))
    lines = ["created_at: 2026-01-01T00:00:00Z", "---"]
    for name, text in sorted(info.items()):
        with open(os.path.join(directory, "info", name), "w") as f:
            f.write(text)
        versions = ",".join(line.split(" ")[0] for line in text.splitlines())
        lines.append(f"{name} {versions} {hashlib.md5(text.encode()).hexdigest()}")
    with open(os.path.join(directory, "versions"), "w") as f:
        f.write("\n".join(lines) + "\n")


def specs_of(lockfile):
    specs, inside = [], False
    for line in lockfile.splitlines():
        if line == "  specs:":
            inside = True
        elif inside and not line:
            break
        elif inside and line.startswith("    ") and not line.startswith("     "):
            specs.append(line.strip())
    return specs


def update_of(edit, lockfile):
    """What an edit updates: None for no update, [] for every gem, else the
    one gem the lockfile locks it picks."""
    if not edit or not edit.startswith("update"):
        return None
    if edit == "update":
        return []
    names = sorted({spec.split(" ")[0] for spec in specs_of(lockfile)})
    return [names[int(edit.split(":")[1]) % len(names)]] if names else []


def run_bundler(case_dir, source, gemfile, app_name="bundler", lockfile=None, whole=False, update=None):
    app = os.path.join(case_dir, app_name)
    os.makedirs(app)
    if lockfile is not None:
        with open(os.path.join(app, "Gemfile.lock"), "w") as f:
            f.write(lockfile)
    with open(os.path.join(app, "Gemfile"), "w") as f:
        f.write(f'source "{source}"\n')
        for name, reqs in gemfile:
            f.write(f'gem "{name}"' + "".join(f', "{r}"' for r in reqs) + "\n")
    env = {
        "PATH": os.environ["PATH"],
        "HOME": os.path.join(case_dir, "home"),
        "BUNDLE_GEMFILE": os.path.join(app, "Gemfile"),
        "BUNDLE_USER_HOME": os.path.join(case_dir, "bundle"),
        "BUNDLE_APP_CONFIG": os.path.join(app, ".bundle"),
        "ORACLE_PLATFORM": "x86_64-linux",
        "ORACLE_RUBY": "4.0.0",
        "ORACLE_RUBYGEMS": "4.0.16",
        "RUBYOPT": f"-r{os.path.join(HERE, 'bundler_oracle.rb')}",
    }
    command = ["bundle", "lock"] + ([] if update is None else ["--update", *update])
    out = subprocess.run(command, cwd=app, env=env, capture_output=True, text=True, timeout=300)
    if out.returncode == 0:
        with open(os.path.join(app, "Gemfile.lock")) as f:
            text = f.read()
            return True, text if whole else specs_of(text)
    return False, (out.stdout + out.stderr).strip()


def run_spinel(spinel, case_dir, source, gemfile, locked=None, update=None):
    deps = {
        "gemfiles": [os.path.join(case_dir, "Gemfile")],
        "source": {"type": "rubygems", "remote": source, "cooldown": None},
        "dependencies": [
            {"name": name, "requirements": reqs or [">= 0"], "platforms": [], "source": None,
             "force_ruby_platform": None}
            for name, reqs in gemfile
        ],
    }
    path = os.path.join(case_dir, "deps.json")
    with open(path, "w") as f:
        json.dump(deps, f)
    extra = []
    if locked is not None:
        lock_path = os.path.join(case_dir, "locked.Gemfile.lock")
        with open(lock_path, "w") as f:
            f.write(locked)
        extra = ["--locked", lock_path, "--lockfile"]
        if update is not None:
            extra.append("--update" + (f"={','.join(update)}" if update else ""))
    out = subprocess.run(
        [spinel, "resolve", path, "--platform", "x86_64-linux",
         "--ruby-version", "4.0.0", "--rubygems-version", "4.0.16", *extra],
        env={**os.environ, "SPINEL_HOME": os.path.join(case_dir, "spinel-home")},
        capture_output=True, text=True, timeout=300,
    )
    if out.returncode == 0:
        return True, out.stdout if locked is not None else out.stdout.split("\n")[:-1]
    return False, out.stderr.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--spinel", required=True)
    parser.add_argument("--cases", type=int, default=300)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--minutes", type=float, default=30, help="stop starting cases after this long")
    parser.add_argument("--out", required=True, help="a scratch directory, emptied first")
    parser.add_argument("--locked", action="store_true", help="relock an edited Gemfile (#79)")
    parser.add_argument("--steps", type=int, default=1, help="with --locked, edits in a row")
    parser.add_argument("--updates", action="store_true", help="with --locked, updates among the edits (#87)")
    args = parser.parse_args()
    spinel = os.path.abspath(args.spinel)
    shutil.rmtree(args.out, ignore_errors=True)
    os.makedirs(args.out)
    root = os.path.abspath(args.out)

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=root, **k)

        def log_message(self, *a):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"

    rng = random.Random(args.seed)
    cases = [random_case(rng) for _ in range(args.cases)]
    edits = None
    if args.locked:
        edits = []
        for info, gemfile in cases:
            chain = []
            for _ in range(args.steps):
                chain.append(edit_gemfile(rng, info, gemfile, args.updates))
                gemfile = chain[-1][1]
            edits.append(chain)
    deadline = time.monotonic() + args.minutes * 60

    def run(i):
        if time.monotonic() > deadline:
            return i, "skipped", None
        info, gemfile = cases[i]
        case_dir = os.path.join(root, f"case{i:04d}")
        write_index(os.path.join(case_dir, "index"), info)
        source = f"{base}/case{i:04d}/index"
        edit = None
        if args.locked:
            first = run_bundler(case_dir, source, gemfile, whole=True)
            if not first[0]:
                shutil.rmtree(case_dir)
                return i, "no-lock", None
            lock, steps = first[1], []
            for step, (edit, edited) in enumerate(edits[i]):
                update = update_of(edit, lock)
                bundler = run_bundler(case_dir, source, edited, f"relock{step}", lock, whole=True, update=update)
                ours = run_spinel(spinel, case_dir, source, edited, locked=lock, update=update)
                steps.append({"edit": edit, "edited": edited, "lock": lock})
                if not bundler[0] or bundler != ours:
                    break
                lock = bundler[1]
            edit = "+".join(s["edit"].split(":")[0] for s in steps)
            gemfile = {"first": gemfile, "steps": steps}
        else:
            bundler = run_bundler(case_dir, source, gemfile)
            ours = run_spinel(spinel, case_dir, source, gemfile)
        if bundler[0] and ours[0]:
            kind = "same" if bundler[1] == ours[1] else "differ"
        elif not bundler[0] and not ours[0]:
            kind = "both-failed"
        else:
            kind = "spinel-over" if ours[0] else "spinel-miss"
        if args.locked and kind not in ("same", "both-failed"):
            edited = gemfile["steps"][-1]["edited"]
            fresh_bundler = run_bundler(case_dir, source, edited, "unlocked")
            fresh_ours = run_spinel(spinel, case_dir, source, edited)
            if fresh_bundler[0] != fresh_ours[0] or (fresh_bundler[0] and fresh_bundler[1] != fresh_ours[1]):
                kind += "-unlocked-too"
        if kind in ("same", "both-failed"):
            shutil.rmtree(case_dir)
        else:
            with open(os.path.join(case_dir, "result.json"), "w") as f:
                json.dump({"gemfile": gemfile, "info": info, "kind": kind,
                           "bundler": bundler, "spinel": ours}, f, indent=2)
        return i, kind, edit

    counts = {}
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for i, kind, edit in pool.map(run, range(args.cases)):
            counts[kind] = counts.get(kind, 0) + 1
            if edit is not None and kind == "same":
                counts[f"same-{edit}"] = counts.get(f"same-{edit}", 0) + 1
            if kind not in ("same", "both-failed", "skipped", "no-lock"):
                print(f"case{i:04d}: {kind}", flush=True)
    server.shutdown()
    print(json.dumps({"seed": args.seed, "cases": args.cases, **counts}, sort_keys=True))
    return 1 if counts.get("spinel-over") else 0


if __name__ == "__main__":
    sys.exit(main())
