//! `require_relative`, as much of it as ruby/spec needs.
//!
//! Almost every spec file opens with `require_relative '../spec_helper'` and
//! `require_relative 'fixtures/classes'`, and the constants those files define
//! are what the examples are written against. Until this module existed the
//! harness never ran those lines, so `BlockSpecs` was never defined and every
//! example in the file reported blocked on a `NameError` whatever the VM could
//! actually do — a measurement bug rather than a behaviour one ([#183]).
//!
//! # This is not the loader [#39] will write
//!
//! There is no `$LOAD_PATH`, no loaded-features table, no `autoload`, and no
//! bytecode cache. `require` of a library name does nothing at all, so an
//! example that needs `mspec` stays blocked, honestly. What is here is the one
//! rule ruby/spec uses: resolve a path against the requiring file's own
//! directory, and evaluate it into the heap before the example runs.
//!
//! # Compiled once per file, evaluated once per example
//!
//! The harness makes a fresh [`Heap`](spinel_vm::Heap) per example, and there
//! are 25,000 of them. Evaluating a fixture per example is unavoidable —
//! constants belong to a heap — but parsing and compiling it is not, so that
//! happens once per spec file and the [`Iseq`]s are shared.
//!
//! [#183]: https://github.com/ar4mirez/spinel/issues/183
//! [#39]: https://github.com/ar4mirez/spinel/issues/39

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use spinel_ast::{Expr, ExprKind, Program};
use spinel_vm::{Iseq, compile};

/// The fixtures one spec file reaches, compiled, in the order Ruby runs them.
pub type Fixtures = Arc<Vec<Fixture>>;

pub struct Fixture {
    /// Kept for the debugger rather than the report: when an example is blocked
    /// on a constant a fixture should have defined, this is the file to open.
    #[allow(dead_code)]
    pub path: PathBuf,
    pub iseq: Arc<Iseq>,
}

/// Compile everything `file` reaches through `require_relative`, depth first.
///
/// A fixture that cannot be read, parsed, or compiled is skipped rather than
/// reported: the example that needed it stays blocked on the constant it could
/// not find, which is the honest reason and the one that ranks the next slice.
#[must_use]
pub fn preload(file: &Path, program: &Program) -> Fixtures {
    let mut seen = HashSet::new();
    seen.insert(key(file));
    let mut out = Vec::new();
    walk(file, program, &mut seen, &mut out);
    Arc::new(out)
}

fn walk(from: &Path, program: &Program, seen: &mut HashSet<PathBuf>, out: &mut Vec<Fixture>) {
    for target in targets(&program.body) {
        let Some(path) = resolve(from, &target) else {
            continue;
        };
        // Within one example a diamond require must not define anything twice,
        // and a cycle must not recurse forever. One set covers both.
        if !seen.insert(key(&path)) {
            continue;
        }
        let Ok(source) = std::fs::read(&path) else {
            continue;
        };
        let parsed = spinel_parse::parse(&source);
        if !parsed.errors.is_empty() {
            continue;
        }
        // Depth first: what a fixture requires has to be defined before the
        // fixture's own body runs, which is the order Ruby gives it.
        walk(&path, &parsed.program, seen, out);
        if let Ok(iseq) = compile::program(&parsed.program) {
            out.push(Fixture {
                path,
                iseq: Arc::new(iseq),
            });
        }
    }
}

/// Every `require_relative "..."` at a file's top level.
///
/// Only the top level, and only a plain string: ruby/spec writes no other
/// shape, and a computed path is one this cannot resolve without running it.
fn targets(body: &[Expr]) -> Vec<String> {
    let mut out = Vec::new();
    for statement in body {
        let ExprKind::Call(call) = &statement.kind else {
            continue;
        };
        if call.receiver.is_some() || &*call.name != "require_relative" {
            continue;
        }
        let [argument] = &call.args[..] else {
            continue;
        };
        if let ExprKind::Str(string) = &argument.kind
            && let [spinel_ast::StrPart::Bytes(bytes)] = &string.parts[..]
            && let Ok(text) = std::str::from_utf8(bytes)
        {
            out.push(text.to_owned());
        }
    }
    out
}

/// Resolve `target` against the directory `from` lives in, adding `.rb`.
fn resolve(from: &Path, target: &str) -> Option<PathBuf> {
    let base = from.parent()?.join(target);
    let with_extension = if base.extension().is_some_and(|e| e == "rb") {
        base.clone()
    } else {
        let mut named = base.clone().into_os_string();
        named.push(".rb");
        PathBuf::from(named)
    };
    if with_extension.is_file() {
        return Some(with_extension);
    }
    base.is_file().then_some(base)
}

/// The identity two paths to the same file share.
fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
