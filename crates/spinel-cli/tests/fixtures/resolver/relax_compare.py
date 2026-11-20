#!/usr/bin/env python3
"""Compare what each try of a resolution relaxes with Bundler's (#816).

An opt-in tool, never run by CI. On `differential.py`'s random cases (the
same seed makes the same cases), Bundler locks each through
`bundler_oracle.rb` with `relax_hook.rb` loaded, which logs what
`Resolver#find_names_to_relax` returns on each try, and `spinel resolve`
resolves it with SPINEL_DEBUG_RELAX set, which prints the same of each of
its tries. A case's outcome is one of differential.py's kinds, and its tries
differ when what some try returns (`unlock` and `pre`, in order), or the
number of tries, is not Bundler's:

  crates/spinel-cli/tests/fixtures/resolver/relax_compare.py \\
      --spinel target/debug/spinel --solver pubgrub-ruby --seed 816 --cases 1000

Bundler's results are read from relax/seed<N>.json beside this file when it
has the case, so a run needs no Bundler for the cached seeds (1, 816, 4242,
9001, 20261120); a case it lacks is locked by Bundler, and --write saves it
there. Prints each case that is not `same` or `both-failed`, or whose tries
differ, and the counts; exits 1 when any case is `spinel-over`, `differ` or
`spinel-miss`, or its tries differ. Written with Bundler 4.0.20 on Ruby
4.0.9.
"""

import argparse
import concurrent.futures
import http.server
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True  # no __pycache__ beside the fixtures
import differential  # noqa: E402

RELAX = re.compile(r"^find_names_to_relax: (.*)$", re.M)


def relaxed(line):
    """One try's find_names_to_relax result, as logged, in its order."""
    names = json.loads(line)
    return {"unlock": names["unlock"], "pre": names["pre"]}


def bundler(case_dir, source, gemfile):
    """Bundler's outcome and, for each try, what find_names_to_relax returned."""
    app = os.path.join(case_dir, "bundler")
    os.makedirs(app)
    with open(os.path.join(app, "Gemfile"), "w") as f:
        f.write(f'source "{source}"\n')
        for name, reqs in gemfile:
            f.write(f'gem "{name}"' + "".join(f', "{r}"' for r in reqs) + "\n")
    log = os.path.join(case_dir, "relax.log")
    env = {
        "PATH": os.environ["PATH"],
        "HOME": os.path.join(case_dir, "home"),
        "BUNDLE_GEMFILE": os.path.join(app, "Gemfile"),
        "BUNDLE_USER_HOME": os.path.join(case_dir, "bundle"),
        "BUNDLE_APP_CONFIG": os.path.join(app, ".bundle"),
        "ORACLE_PLATFORM": "x86_64-linux",
        "ORACLE_RUBY": "4.0.0",
        "ORACLE_RUBYGEMS": "4.0.16",
        "RELAX_LOG": log,
        "RUBYOPT": f"-r{os.path.join(HERE, 'bundler_oracle.rb')} -r{os.path.join(HERE, 'relax_hook.rb')}",
    }
    out = subprocess.run(["bundle", "lock"], cwd=app, env=env, capture_output=True, text=True, timeout=300)
    tries = []
    if os.path.exists(log):
        with open(log) as f:
            tries = [relaxed(line) for line in f]
    if out.returncode == 0:
        with open(os.path.join(app, "Gemfile.lock")) as f:
            return {"ok": True, "specs": differential.specs_of(f.read()), "tries": tries}
    return {"ok": False, "specs": None, "tries": tries}


def spinel(binary, case_dir, source, gemfile, solver):
    """Spinel's outcome and, for each try, what find_names_to_relax returned."""
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
    extra = ["--solver", solver] if solver else []
    out = subprocess.run(
        [binary, "resolve", path, "--platform", "x86_64-linux", "--ruby-version", "4.0.0",
         "--rubygems-version", "4.0.16", *extra],
        env={**os.environ, "SPINEL_HOME": os.path.join(case_dir, "spinel-home"), "SPINEL_DEBUG_RELAX": "1"},
        capture_output=True, text=True, timeout=300,
    )
    tries = [relaxed(line) for line in RELAX.findall(out.stderr)]
    if out.returncode == 0:
        return {"ok": True, "specs": out.stdout.split("\n")[:-1], "tries": tries}
    return {"ok": False, "specs": None, "tries": tries}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--spinel", required=True)
    parser.add_argument("--cases", type=int, default=300)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--solver", help="spinel resolve --solver: pubgrub or pubgrub-ruby")
    parser.add_argument("--write", action="store_true", help="save Bundler's results to relax/seed<N>.json")
    args = parser.parse_args()
    binary = os.path.abspath(args.spinel)
    cache_file = os.path.join(HERE, "relax", f"seed{args.seed}.json")
    cache = {}
    if os.path.exists(cache_file):
        with open(cache_file) as f:
            cache = json.load(f)
    root = tempfile.mkdtemp(prefix="spinel-relax-compare.")

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=root, **k)

        def log_message(self, *a):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    rng = random.Random(args.seed)
    cases = [differential.random_case(rng) for _ in range(args.cases)]

    def run(i):
        info, gemfile = cases[i]
        case_dir = os.path.join(root, f"case{i:04d}")
        differential.write_index(os.path.join(case_dir, "index"), info)
        source = f"{base}/case{i:04d}/index"
        theirs = cache.get(str(i)) or bundler(case_dir, source, gemfile)
        ours = spinel(binary, case_dir, source, gemfile, args.solver)
        shutil.rmtree(case_dir)
        if theirs["ok"] and ours["ok"]:
            kind = "same" if theirs["specs"] == ours["specs"] else "differ"
        elif not theirs["ok"] and not ours["ok"]:
            kind = "both-failed"
        else:
            kind = "spinel-over" if ours["ok"] else "spinel-miss"
        return i, kind, theirs, ours

    counts = {}
    tries_differ = 0
    try:
        with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
            for i, kind, theirs, ours in pool.map(run, range(args.cases)):
                cache[str(i)] = theirs
                counts[kind] = counts.get(kind, 0) + 1
                differ = theirs["tries"] != ours["tries"]
                tries_differ += differ
                if kind not in ("same", "both-failed") or differ:
                    print(f"case{i:04d}: {kind}{' tries-differ' if differ else ''} "
                          f"bundler={theirs['tries']} spinel={ours['tries']}", flush=True)
    finally:
        server.shutdown()
        shutil.rmtree(root, ignore_errors=True)
    if args.write:
        os.makedirs(os.path.dirname(cache_file), exist_ok=True)
        with open(cache_file, "w") as f:
            json.dump({k: cache[k] for k in sorted(cache, key=int)}, f, separators=(",", ":"))
            f.write("\n")
    print(json.dumps({"seed": args.seed, "cases": args.cases, **counts, "tries-differ": tries_differ},
                     sort_keys=True))
    bad = tries_differ + sum(counts.get(k, 0) for k in ("spinel-over", "differ", "spinel-miss"))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
