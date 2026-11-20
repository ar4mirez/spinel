#!/usr/bin/env python3
"""Compare the solver's decisions with Bundler's, case by case (#816).

An opt-in tool, never run by CI. For each case directory (as `bundle_case.sh`
takes: `info/<gem>` and `Gemfile.gems`), the index is served on 127.0.0.1;
Bundler locks it through `bundler_oracle.rb` with DEBUG_RESOLVER set, and
`spinel resolve --solver pubgrub-ruby` resolves it with DEBUG_RESOLVER set.
Both print pubgrub-ruby's log; the first try of each is kept (Bundler's
retries, after `find_names_to_relax`, start again at `fact: (root
dependency)`), and the lines that record decisions are compared:

  attempting <gem> <version>   the strategy's choice
  selected <gem> <version>     a decision made
  derived: <term>              a term unit propagation derived

With --all every line pubgrub-ruby logs is compared (facts, conflicts,
backtracking, conflict resolution), the source's URL replaced by @SOURCE@
and Bundler's `or installed locally` after it dropped: `bundle lock` names
the source as one that may also install locally, which Spinel's does not
(docs/package-manager.md, "Resolver").

  crates/spinel-cli/tests/fixtures/resolver/decision_log.py \\
      --spinel target/debug/spinel crates/spinel-cli/tests/fixtures/resolver/oracle/*

Prints `same` or the first lines that differ for each case, and exits 1 when
any differs. With --write, Bundler's first try is also written, every line
(less a trailing space), to `first_try.log` in each case directory: what the
resolver's unit tests hold the port to. Written with Bundler 4.0.20 on Ruby 4.0.9.
"""

import argparse
import difflib
import http.server
import json
import os
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

DECISIONS = re.compile(r"^(attempting |selected |derived: )")
PREFIXES = r"(fact: |derived: |attempting |selected |conflict: |knew: |backtracking to |! |Solution found|\* )"
PUBGRUB = re.compile(f"^{PREFIXES}")
# `attempting g3 ` with no version, run into the line after it.
RUN_IN = re.compile(f"^(attempting \\S+ )(?={PREFIXES})", re.M)


def read_case(case):
    """The case's /info texts and Gemfile dependencies."""
    info = {}
    for name in sorted(os.listdir(os.path.join(case, "info"))):
        with open(os.path.join(case, "info", name)) as f:
            text = "".join(line for line in f if line.strip() != "---")
        info[name] = text
    gemfile = []
    with open(os.path.join(case, "Gemfile.gems")) as f:
        for line in f:
            words = re.findall(r'"([^"]*)"', line)
            if line.strip().startswith("gem ") and words:
                gemfile.append((words[0], words[1:]))
    return info, gemfile


def log_lines(text, source):
    """pubgrub-ruby's lines of the first try, one per line.

    Bundler's shell does not end a line that ends in a space, so a version
    the strategy found none of (`attempting g3 `) runs into the next line."""
    text = text.replace(source, "@SOURCE@").replace("@SOURCE@ or installed locally", "@SOURCE@")
    text = RUN_IN.sub(r"\1\n", text)
    lines = [line for line in text.split("\n") if PUBGRUB.match(line)]
    starts = [i for i, line in enumerate(lines) if line == "fact: (root dependency)"]
    if len(starts) > 1:
        lines = lines[: starts[1]]
    return lines


def bundler_log(case_dir, source, gemfile):
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
        "DEBUG_RESOLVER": "1",
    }
    out = subprocess.run(["bundle", "lock"], cwd=app, env=env, capture_output=True, text=True, timeout=300)
    return out.stdout + out.stderr


def spinel_log(spinel, case_dir, source, gemfile):
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
        [spinel, "resolve", path, "--solver", "pubgrub-ruby", "--platform", "x86_64-linux",
         "--ruby-version", "4.0.0", "--rubygems-version", "4.0.16"],
        env={**os.environ, "SPINEL_HOME": os.path.join(case_dir, "spinel-home"), "DEBUG_RESOLVER": "1"},
        capture_output=True, text=True, timeout=300,
    )
    return out.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--spinel", required=True)
    parser.add_argument("--all", action="store_true", help="compare every line, not only decisions")
    parser.add_argument("--write", action="store_true", help="write Bundler's first try to <case>/first_try.log")
    parser.add_argument("cases", nargs="+", help="case directories: info/<gem> and Gemfile.gems")
    args = parser.parse_args()
    spinel = os.path.abspath(args.spinel)
    root = tempfile.mkdtemp(prefix="spinel-decision-log.")

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=root, **k)

        def log_message(self, *a):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    differing = 0
    try:
        for n, case in enumerate(args.cases):
            info, gemfile = read_case(case)
            case_dir = os.path.join(root, f"case{n}")
            differential.write_index(os.path.join(case_dir, "index"), info)
            source = f"{base}/case{n}/index/"
            ours = log_lines(spinel_log(spinel, case_dir, source, gemfile), source)
            theirs = log_lines(bundler_log(case_dir, source, gemfile), source)
            if args.write:
                with open(os.path.join(case, "first_try.log"), "w") as f:
                    f.write("".join(f"{line.rstrip()}\n" for line in theirs))
            if not args.all:
                ours = [line for line in ours if DECISIONS.match(line)]
                theirs = [line for line in theirs if DECISIONS.match(line)]
            name = os.path.basename(os.path.normpath(case))
            if ours == theirs:
                print(f"{name}: same ({len(ours)} lines)")
                continue
            differing += 1
            print(f"{name}: differs")
            diff = difflib.unified_diff(theirs, ours, "bundler", "spinel", lineterm="", n=3)
            for line in list(diff)[:40]:
                print(f"  {line}")
    finally:
        server.shutdown()
        shutil.rmtree(root, ignore_errors=True)
    return 1 if differing else 0


if __name__ == "__main__":
    sys.exit(main())
