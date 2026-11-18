//! The native-extension ABI (#103), end to end: `tests/fixtures/ext/greeter`
//! — the worked example of `spinel-ext`'s documentation — run against a
//! booted heap, both compiled into this test and built as a real `cdylib`
//! and opened, and an extension built against an ABI no `spinel` loads,
//! refused before any of its code runs.
//!
//! `require` finding a `.spinel` file is #104, so these call the loader's
//! half — `Extension::open`, the check, `Extension::init` — directly.

#![cfg(not(miri))]

use std::path::{Path, PathBuf};
use std::process::Command;

use spinel_ext::abi::{Header, RawContext, Status};
use spinel_ext::{Context, Error as ExtError, Result as ExtResult};
use spinel_vm::interp::ext::{Extension, LoadError, check};
use spinel_vm::{Error, HandleScope, Heap, compile, interp};

#[path = "fixtures/ext/greeter/src/lib.rs"]
mod greeter;

/// Run `source` in `scope`, answering its value's `inspect`.
fn ruby(scope: &mut HandleScope<'_>, source: &str) -> Result<String, Error> {
    let parsed = spinel_parse::parse_file("(ext)", source.as_bytes());
    assert!(parsed.errors.is_empty(), "{source:?}: {:?}", parsed.errors);
    let iseq = compile::program(&parsed.program).expect("the test's Ruby compiles");
    let mut frame = interp::Frame::new(iseq.locals.len());
    let value = interp::eval_in(scope, &mut frame, &iseq)?;
    Ok(interp::inspect(scope, value))
}

/// `source`'s value, which must not raise.
fn eval(scope: &mut HandleScope<'_>, source: &str) -> String {
    ruby(scope, source).unwrap_or_else(|error| panic!("{source}: {error}"))
}

/// A booted heap's scope with `extension` initialised in it.
fn with_extension(extension: &Extension, test: impl FnOnce(&mut HandleScope<'_>)) {
    let mut heap = Heap::new();
    let mut scope = heap.scope();
    scope.bootstrap();
    spinel_core::boot(&mut scope);
    extension
        .init(&mut scope)
        .expect("the extension's init runs");
    test(&mut scope);
}

fn in_process() -> Extension {
    Extension::new(
        "greeter (in process)",
        &greeter::spinel_ext_header,
        greeter::spinel_ext_init,
    )
    .expect("the header names this ABI")
}

/// The worked example's Ruby half, as the documentation shows it.
fn the_worked_example_answers(scope: &mut HandleScope<'_>) {
    assert_eq!(eval(scope, r#"Greeter.hello("Ruby")"#), r#""Hello, Ruby!""#);
    assert_eq!(eval(scope, "Greeter.add(2, 40)"), "42");
    assert_eq!(eval(scope, "Greeter::Counter.new.zero?"), "true");
    assert_eq!(
        eval(
            scope,
            r#"begin; Greeter.hello(""); rescue ArgumentError => e; e.message; end"#
        ),
        r#""name is empty""#
    );
    assert_eq!(
        eval(
            scope,
            "begin; Greeter.add(1); rescue ArgumentError => e; e.message; end"
        ),
        r#""wrong number of arguments (given 1, expected 2)""#
    );
}

#[test]
fn the_worked_example_runs_in_process() {
    with_extension(&in_process(), the_worked_example_answers);
}

#[test]
fn values_convert_both_ways() {
    with_extension(&in_process(), |scope| {
        // A bignum in and out, as long as it fits an i64.
        assert_eq!(eval(scope, "Greeter.add(2**62, 1)"), "4611686018427387905");
        assert_eq!(
            eval(scope, "Greeter.add(-2**62, -1)"),
            "-4611686018427387905"
        );
        assert_eq!(
            eval(scope, r#"Greeter.hello("wörld")"#),
            r#""Hello, wörld!""#
        );
        assert_eq!(
            eval(scope, r#"Greeter.hello("x").encoding.to_s"#),
            r#""UTF-8""#
        );
        assert_eq!(eval(scope, "Greeter.nothing"), "nil");
        for (value, kind) in [
            ("nil", "nil"),
            ("true", "true"),
            ("false", "false"),
            ("1", "integer"),
            ("2**70", "integer"),
            ("1.5", "float"),
            (r#""s""#, "string"),
            (":s", "symbol"),
            ("[1]", "array"),
            ("{a: 1}", "hash"),
            ("Kernel", "module"),
            ("String", "class"),
            ("Object.new", "object"),
        ] {
            assert_eq!(
                eval(scope, &format!("Greeter.kind({value})")),
                format!("{kind:?}"),
                "Greeter.kind({value})"
            );
        }
        assert_eq!(eval(scope, "Greeter.truthy?(0)"), "true");
        assert_eq!(eval(scope, "Greeter.truthy?(false)"), "false");
        assert_eq!(eval(scope, "Greeter.truthy?(nil)"), "false");
    });
}

#[test]
fn arguments_are_counted_and_keywords_arrive_as_a_hash() {
    with_extension(&in_process(), |scope| {
        assert_eq!(eval(scope, "Greeter.count"), "0");
        assert_eq!(eval(scope, "Greeter.count(1, 2, 3)"), "3");
        assert_eq!(eval(scope, "Greeter.count(1, a: 2)"), "2");
        assert_eq!(eval(scope, "Greeter.method(:add).arity"), "2");
        assert_eq!(eval(scope, "Greeter.method(:count).arity"), "-1");
        assert_eq!(
            eval(scope, "Greeter::Counter.instance_method(:zero?).arity"),
            "0"
        );
        assert_eq!(eval(scope, "Greeter.respond_to?(:hello)"), "true");
    });
}

#[test]
fn a_failure_is_a_ruby_exception() {
    with_extension(&in_process(), |scope| {
        let rescued = |scope: &mut HandleScope<'_>, call: &str| {
            eval(
                scope,
                &format!("begin; {call}; :none; rescue Exception => e; [e.class, e.message]; end"),
            )
        };
        assert_eq!(
            rescued(scope, r#"Greeter.add("1", 2)"#),
            r#"[TypeError, "no implicit conversion of String into Integer"]"#
        );
        assert_eq!(
            rescued(scope, "Greeter.hello(nil)"),
            r#"[TypeError, "no implicit conversion of nil into String"]"#
        );
        assert_eq!(
            rescued(scope, "Greeter.add(2**63 - 1, 1)"),
            r#"[RangeError, "sum overflows a 64-bit integer"]"#
        );
        assert_eq!(
            rescued(scope, "Greeter.add(2**64, 1)"),
            r#"[RangeError, "bignum too big to convert into 'long'"]"#
        );
        assert_eq!(
            rescued(scope, r#"Greeter.raise_with(Greeter::Error, "custom")"#),
            r#"[Greeter::Error, "custom"]"#
        );
        assert_eq!(
            eval(
                scope,
                r#"begin; Greeter.raise_with(Greeter::Error, "x"); rescue StandardError; :standard; end"#
            ),
            ":standard"
        );
        assert_eq!(
            rescued(scope, r#"Greeter.raise_with(String, "x")"#),
            r#"[TypeError, "exception class/object expected"]"#
        );
        assert_eq!(
            rescued(scope, "Greeter.panic"),
            r#"[RuntimeError, "native extension panicked: the greeter fell over"]"#
        );
        assert_eq!(
            rescued(scope, "Greeter.missing"),
            r#"[NameError, "uninitialized constant Greeter::NoSuchError"]"#
        );
        // One the program does not rescue ends it, as any exception does.
        match ruby(scope, r#"Greeter.hello("")"#) {
            Err(Error::Uncaught { class, message }) => {
                assert_eq!(
                    (class.as_str(), message.as_str()),
                    ("ArgumentError", "name is empty")
                );
            }
            other => panic!("expected an uncaught ArgumentError, got {other:?}"),
        }
        // And the heap is still good for the next call.
        assert_eq!(eval(scope, "Greeter.add(1, 1)"), "2");
    });
}

fn raising_init(_cx: &mut Context<'_>) -> ExtResult<()> {
    Err(ExtError::new("ArgumentError", "no configuration"))
}

unsafe extern "C" fn raising_entry(cx: *mut RawContext) -> Status {
    // SAFETY: the host's context, passed on.
    unsafe { spinel_ext::__init(cx, raising_init) }
}

/// An init that finds the host's table shorter than the one it was built
/// against, as an extension built by a newer `spinel-ext` of the same ABI
/// would.
unsafe extern "C" fn too_short_entry(_cx: *mut RawContext) -> Status {
    Status::ABI_MISMATCH
}

#[test]
fn an_init_that_raises_or_finds_the_table_short_is_an_error() {
    let header = Header::new("configured", "1.0.0");
    let mut heap = Heap::new();
    let mut scope = heap.scope();
    scope.bootstrap();
    spinel_core::boot(&mut scope);
    let raising = Extension::new("configured.spinel", &header, raising_entry).expect("ABI 1");
    match raising.init(&mut scope) {
        Err(Error::Uncaught { class, message }) => {
            assert_eq!(
                (class.as_str(), message.as_str()),
                ("ArgumentError", "no configuration")
            );
        }
        other => panic!("expected the init's ArgumentError, got {other:?}"),
    }
    let short = Extension::new("configured.spinel", &header, too_short_entry).expect("ABI 1");
    match short.init(&mut scope) {
        Err(Error::Raise { class, message }) => {
            assert_eq!(class, "LoadError");
            assert_eq!(
                message,
                format!(
                    "the extension configured 1.0.0 (configured.spinel) needs entries of \
                     spinel-ext ABI {} that this spinel {} does not provide; upgrade spinel",
                    spinel_ext::ABI,
                    spinel_vm::ENGINE_VERSION
                )
            );
        }
        other => panic!("expected a LoadError, got {other:?}"),
    }
}

#[test]
fn a_mismatched_abi_is_refused_with_what_to_do() {
    let path = Path::new("/gems/greeter-0.1.0/lib/greeter.spinel");
    let running = spinel_ext::ABI;
    let spinel = spinel_vm::ENGINE_VERSION;

    let old = Header {
        abi: 0,
        ..Header::new("greeter", "0.1.0")
    };
    let error = check(path, &old).expect_err("ABI 0 predates the first API");
    assert_eq!(
        error.to_string(),
        format!(
            "the extension greeter 0.1.0 ({}) was built against spinel-ext ABI 0, older than \
             the ABI {running} this spinel {spinel} loads; rebuild it against this \
             spinel by reinstalling the gem that ships it",
            path.display()
        )
    );
    let new = Header {
        abi: running + 1,
        ..Header::new("greeter", "0.1.0")
    };
    let error = check(path, &new).expect_err("a newer ABI is not this one");
    assert_eq!(
        error.to_string(),
        format!(
            "the extension greeter 0.1.0 ({}) was built against spinel-ext ABI {}, newer than \
             the ABI {running} this spinel {spinel} loads; upgrade spinel, or install a version \
             of the gem built for ABI {running}",
            path.display(),
            running + 1
        )
    );
    let garbage = Header {
        magic: *b"NOTSPNL\0",
        ..Header::new("greeter", "0.1.0")
    };
    assert!(matches!(
        check(path, &garbage),
        Err(LoadError::NotAnExtension { .. })
    ));
    // `Extension::new` checks too, so a refused header never pairs with its
    // init.
    assert!(Extension::new(path, &old, greeter::spinel_ext_init).is_err());
    // The error raises as a LoadError, carrying the same message.
    match Error::from(check(path, &old).unwrap_err()) {
        Error::Raise { class, message } => {
            assert_eq!(class, "LoadError");
            assert!(message.contains("ABI 0"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

/// Build the fixture crate `name` as a `cdylib` outside the source tree,
/// against this checkout's `spinel-ext`, and answer the library's path.
fn build(name: &str) -> PathBuf {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ext");
    let spinel_ext = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spinel-ext");
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ext-fixtures");
    let crate_dir = root.join(name);
    std::fs::create_dir_all(crate_dir.join("src")).expect("a directory for the copy");
    let manifest = std::fs::read_to_string(fixtures.join(name).join("Cargo.toml"))
        .expect("the fixture's manifest")
        .replace(
            r#"path = "../../../../../spinel-ext""#,
            &format!("path = {:?}", spinel_ext.display().to_string()),
        );
    std::fs::write(crate_dir.join("Cargo.toml"), manifest).expect("the manifest copied");
    std::fs::copy(
        fixtures.join(name).join("src/lib.rs"),
        crate_dir.join("src/lib.rs"),
    )
    .expect("the source copied");
    let target = root.join("target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "--offline", "--jobs", "2"])
        .arg("--manifest-path")
        .arg(crate_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "building the {name} fixture failed");
    target.join("debug").join(format!(
        "{}{name}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ))
}

#[test]
fn a_built_extension_loads_and_a_stale_one_is_refused_unrun() {
    let greeter = build("greeter");
    let extension = Extension::open(&greeter).expect("the greeter is ABI 1");
    assert_eq!(extension.header().name(), "greeter");
    assert_eq!(extension.header().version(), "0.1.0");
    assert_eq!(extension.header().abi, spinel_ext::ABI);
    with_extension(&extension, the_worked_example_answers);
    with_extension(&extension, |scope| {
        assert_eq!(
            eval(
                scope,
                "begin; Greeter.panic; rescue RuntimeError => e; e.message; end"
            ),
            r#""native extension panicked: the greeter fell over""#
        );
    });

    // Its init aborts the process, so getting past this line is the proof
    // that a refused extension runs none of its code.
    let stale = build("stale");
    let error = Extension::open(&stale).expect_err("ABI 0 is refused");
    assert_eq!(
        error,
        LoadError::Abi {
            path: stale.clone(),
            name: "stale".to_owned(),
            version: "0.3.0".to_owned(),
            built: 0,
        }
    );
    assert!(
        error.to_string().starts_with(&format!(
            "the extension stale 0.3.0 ({}) was built",
            stale.display()
        )),
        "{error}"
    );

    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-extension.spinel");
    assert!(matches!(
        Extension::open(&missing),
        Err(LoadError::Open { .. })
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn a_library_that_is_not_an_extension_is_refused() {
    let error = Extension::open(Path::new("libc.so.6")).expect_err("libc has no header");
    assert!(
        matches!(&error, LoadError::NotAnExtension { reason, .. } if reason == "it exports no spinel_ext_header"),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("is not a Spinel extension"),
        "{error}"
    );
}
