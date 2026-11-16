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


def run_bundler(case_dir, source, gemfile):
    app = os.path.join(case_dir, "bundler")
    os.makedirs(app)
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
    out = subprocess.run(["bundle", "lock"], cwd=app, env=env, capture_output=True, text=True, timeout=300)
    if out.returncode == 0:
        with open(os.path.join(app, "Gemfile.lock")) as f:
            return True, specs_of(f.read())
    return False, (out.stdout + out.stderr).strip()


def run_spinel(spinel, case_dir, source, gemfile):
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
    out = subprocess.run(
        [spinel, "resolve", path, "--platform", "x86_64-linux",
         "--ruby-version", "4.0.0", "--rubygems-version", "4.0.16"],
        env={**os.environ, "SPINEL_HOME": os.path.join(case_dir, "spinel-home")},
        capture_output=True, text=True, timeout=300,
    )
    if out.returncode == 0:
        return True, out.stdout.split("\n")[:-1]
    return False, out.stderr.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--spinel", required=True)
    parser.add_argument("--cases", type=int, default=300)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--minutes", type=float, default=30, help="stop starting cases after this long")
    parser.add_argument("--out", required=True, help="a scratch directory, emptied first")
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
    deadline = time.monotonic() + args.minutes * 60

    def run(i):
        if time.monotonic() > deadline:
            return i, "skipped", None
        info, gemfile = cases[i]
        case_dir = os.path.join(root, f"case{i:04d}")
        write_index(os.path.join(case_dir, "index"), info)
        source = f"{base}/case{i:04d}/index"
        bundler = run_bundler(case_dir, source, gemfile)
        ours = run_spinel(spinel, case_dir, source, gemfile)
        if bundler[0] and ours[0]:
            kind = "same" if bundler[1] == ours[1] else "differ"
        elif not bundler[0] and not ours[0]:
            kind = "both-failed"
        else:
            kind = "spinel-over" if ours[0] else "spinel-miss"
        if kind in ("same", "both-failed"):
            shutil.rmtree(case_dir)
        else:
            with open(os.path.join(case_dir, "result.json"), "w") as f:
                json.dump({"gemfile": gemfile, "info": info, "kind": kind,
                           "bundler": bundler, "spinel": ours}, f, indent=2)
        return i, kind, None

    counts = {}
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for i, kind, _ in pool.map(run, range(args.cases)):
            counts[kind] = counts.get(kind, 0) + 1
            if kind not in ("same", "both-failed", "skipped"):
                print(f"case{i:04d}: {kind}", flush=True)
    server.shutdown()
    print(json.dumps({"seed": args.seed, "cases": args.cases, **counts}, sort_keys=True))
    return 1 if counts.get("spinel-over") else 0


if __name__ == "__main__":
    sys.exit(main())
