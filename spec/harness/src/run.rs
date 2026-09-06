//! Running an example.
//!
//! This is the seam [#5](https://github.com/ar4mirez/spinel/issues/5) left
//! marked `// ponytail:` — "the whole VM-shaped hole". It is filled here as far
//! as [#10](https://github.com/ar4mirez/spinel/issues/10) reaches: an example
//! whose every statement compiles is *run*, and one that mentions anything the
//! compiler cannot mean yet stays `blocked`.
//!
//! # mspec's DSL lives here, not in the VM
//!
//! The VM knows nothing about `should`. `spinel-vm` compiles Ruby and runs
//! bytecode; that is all. What this module does is recognise the one matcher
//! shape that carries `language/` — `.should ==`, 201 of the calls in the five
//! files this slice targets and 4,071 across the directory — split it into its
//! two halves, and compile each half as an ordinary expression.
//!
//! Both halves then run **in one frame**, which is what makes an example's
//! locals survive from its first statement to its last without the VM having a
//! concept of a test. Equality is [`interp::ruby_eq`], the same function
//! `Insn::BinOp` uses, so this cannot pass an example the VM would fail.
//!
//! Everything else — `should be_nil`, `should.raise`, `should_receive` — needs
//! matchers that need method dispatch, and reports itself as the reason it was
//! blocked so the next slice is chosen from data.

use spinel_ast::{CallFlags, Expr, ExprKind, Name};
use spinel_vm::class::Builtin;
use spinel_vm::{ClassId, Definition, HandleScope, Heap, Native, Payload, Value, compile, interp};

use crate::discover::Example;
use crate::loader::Fixtures;

/// What became of one example.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It ran, and every expectation in it held.
    Passed,
    /// It ran, and an expectation did not hold. A real disagreement with Ruby.
    Failed(String),
    /// A guard excluded it, or the harness would have had to guess.
    Skipped,
    /// Nothing here can run it yet, and the reason names what is missing.
    Blocked(String),
}

/// One statement of an example body, as this harness reads it.
enum Statement<'a> {
    /// `<subject>.should == <expected>`, or `should_not`.
    Compare {
        subject: &'a Expr,
        expected: &'a Expr,
        negated: bool,
    },
    /// `<subject>.should.raise(<class>)`, or `should_not`.
    Raises {
        subject: &'a Expr,
        /// The class the example named. `should_not.raise` usually names none.
        expected: Option<&'a Expr>,
        negated: bool,
    },
    /// Anything else: run it for its effect and discard the value.
    Effect(&'a Expr),
}

/// Why an expression did not produce a value.
///
/// The two are not the same and the report must not merge them: a construct the
/// compiler cannot mean is a gap in Spinel, and a raise is Ruby behaviour that
/// [`Statement::Raises`] is specifically there to *check*.
enum Stop {
    /// The compiler cannot mean it yet.
    Unsupported(String),
    /// The interpreter stopped. Which kind matters — see [`interp::Error`].
    Stopped(interp::Error),
}

impl Stop {
    fn reason(&self) -> String {
        match self {
            Stop::Unsupported(why) => why.clone(),
            Stop::Stopped(error) => error.to_string(),
        }
    }
}

/// Recognise `x.should == y` and `x.should_not == y`.
///
/// Ruby parses that as `(x.should) == y`, so the shape is a binary `==` whose
/// receiver is a no-argument `should` call. Any other matcher — `should be_nil`,
/// `should.raise` — is a different shape and falls through to `Effect`, where it
/// blocks on the method call it is.
fn classify(expr: &Expr) -> Statement<'_> {
    if let ExprKind::Call(outer) = &expr.kind
        && &*outer.name == "=="
        && outer.args.len() == 1
        && outer.block.is_none()
        && let Some(receiver) = &outer.receiver
        && let ExprKind::Call(inner) = &receiver.kind
        && inner.args.is_empty()
        && inner.block.is_none()
        && let Some(subject) = &inner.receiver
        && let negated = match &*inner.name {
            "should" => false,
            "should_not" => true,
            _ => return Statement::Effect(expr),
        }
    {
        return Statement::Compare {
            subject,
            expected: &outer.args[0],
            negated,
        };
    }
    // `x.should.raise(C)` — mspec spells it this way, not `should_raise`, and
    // it is the matcher the corpus leans on hardest: 533 calls in `language/`
    // and 6,222 across the corpus, more than the next four together.
    if let ExprKind::Call(outer) = &expr.kind
        && &*outer.name == "raise"
        && outer.block.is_none()
        && let Some(receiver) = &outer.receiver
        && let ExprKind::Call(inner) = &receiver.kind
        && inner.args.is_empty()
        && inner.block.is_none()
        && let Some(subject) = &inner.receiver
        && let negated = match &*inner.name {
            "should" => false,
            "should_not" => true,
            _ => return Statement::Effect(expr),
        }
    {
        return Statement::Raises {
            subject,
            expected: outer.args.first(),
            negated,
        };
    }
    Statement::Effect(expr)
}

/// `<subject>.call`, as an expression the compiler can be handed.
///
/// The subject of `should.raise` is a `Proc` — `-> { ... }` almost always, but
/// sometimes a local holding one — and what the matcher asserts on is what
/// happens when it *runs*. Synthesising the call rather than reaching inside a
/// lambda literal is what makes both shapes work through one path.
fn call_of(subject: &Expr) -> Expr {
    Expr::new(
        subject.span,
        ExprKind::Call(Box::new(spinel_ast::Call {
            receiver: Some(subject.clone()),
            name: "call".into(),
            name_span: subject.span,
            args: Vec::new(),
            block: None,
            flags: CallFlags {
                has_parens: true,
                ..CallFlags::default()
            },
        })),
    )
}

/// Compile and run one example.
///
/// A fresh [`Heap`] per example, because examples must not see each other's
/// objects and there is no other isolation yet. Bootstrapping one is a few
/// hundred allocations; the whole `language/` corpus runs in well under a
/// second, so the simple thing is also the fast enough thing.
pub fn run(example: &Example, fixtures: &Fixtures) -> Outcome {
    if example.skipped.is_some() {
        return Outcome::Skipped;
    }
    // An `it` with no block is mspec's pending marker, already skipped above;
    // an empty body that reached here is an example that asserts nothing, which
    // mspec passes.
    let mut heap = Heap::new();
    let mut frame = interp::Frame::new(0);
    let mut scope = heap.scope();
    scope.bootstrap();
    // The Ruby half of the core library. `bootstrap` makes the classes; this
    // fills in their methods, and without it every example that calls one is
    // blocked on a method that exists in `core/*.rb`.
    spinel_core::boot(&mut scope);

    // The fixture files this spec file `require_relative`s, in Ruby's order
    // (#183).
    //
    // A fixture that raises is left where it stopped, and whatever it managed
    // to define stays defined. That is deliberate but not obviously safe: a
    // half-built class can answer questions Ruby's version would refuse, and an
    // example running against one can pass for the wrong reason. Blocking every
    // example whose fixtures did not all finish was measured and costs 261 of
    // the 263 examples this slice gained, because fixtures raise part way
    // constantly and almost always past the part the example needed.
    //
    // What makes the lenient rule honest is `scripts/verify-passes.rb`, which
    // re-runs every claimed pass on real Ruby. It found exactly one example
    // passing off a partial fixture, that turned out to be an arity bug in
    // `Exception.new` rather than a loading one, and reports none now. If it
    // ever reports another, this is the first place to look.
    for fixture in fixtures.iter() {
        let mut fixture_frame = interp::Frame::new(0);
        let _ = interp::eval_in(&mut scope, &mut fixture_frame, &fixture.iseq);
    }

    install_scratch_pad(&mut scope, &mut frame);

    // One slot map for the whole example: every statement is compiled against
    // it and hands back the version it grew, so `a` is the same slot in the
    // statement that writes it and the one that reads it.
    let mut locals: Vec<Name> = example.locals.clone();
    for name in compile::declared_locals(&example.body) {
        if !locals.contains(&name) {
            locals.push(name);
        }
    }

    // Boot and the scratch pad are not the example. Anything they raised for is
    // not what the example swallowed.
    scope.clear_missing_method();

    // Since #170 a missing method is a rescuable raise, so an example can catch
    // one and carry on down a branch Ruby never takes — `a.reject! { raise }`
    // under a `rescue StandardError` is four of these in `core/array/`. It can
    // also reach a `should raise_error` matcher as the wrong class. Either way
    // the failure that follows is about Spinel's gap, not about Ruby.
    //
    // The VM cannot tell that gap from a program's own `NoMethodError`, and
    // neither can this: what it can say is that a failure a missing method
    // could explain is not evidence against Ruby. So a failing example that
    // raised for one is reported blocked, naming the method, which is also how
    // the next slice gets chosen.
    //
    // This one rule covers both shapes. A dedicated case in the `raises`
    // matcher was written for the second and then deleted: breaking it
    // deliberately changed nothing, because this already caught everything it
    // did.
    //
    // A *passing* example is left alone. 43 of them raise for a gap and pass
    // anyway, most being specs that assert `NoMethodError` on purpose; every
    // directory where that happens is re-run on CRuby by
    // `scripts/verify-passes.rb`, which is the check that they are real.
    let outcome = (|| {
        let mut ran_something = false;
        for statement in &example.body {
            match classify(statement) {
                Statement::Compare {
                    subject,
                    expected,
                    negated,
                } => {
                    let actual = match eval(&mut scope, &mut frame, &mut locals, subject) {
                        Ok(value) => value,
                        Err(stop) => return Outcome::Blocked(stop.reason()),
                    };
                    let wanted = match eval(&mut scope, &mut frame, &mut locals, expected) {
                        Ok(value) => value,
                        Err(stop) => return Outcome::Blocked(stop.reason()),
                    };
                    let equal = match interp::ruby_eq(&mut scope, actual, wanted) {
                        Ok(equal) => equal,
                        Err(why) => return Outcome::Blocked(why.to_string()),
                    };
                    if equal == negated {
                        let (actual, wanted) = (
                            interp::inspect(&mut scope, actual),
                            interp::inspect(&mut scope, wanted),
                        );
                        let expectation = if negated {
                            "should not equal"
                        } else {
                            "should equal"
                        };
                        return Outcome::Failed(format!("{actual} {expectation} {wanted}"));
                    }
                    ran_something = true;
                }
                Statement::Raises {
                    subject,
                    expected,
                    negated,
                } => {
                    // The class is evaluated first, and before the subject runs, so
                    // an example naming a class Spinel has never heard of is
                    // blocked rather than judged.
                    let wanted = match expected {
                        Some(expr) => match eval(&mut scope, &mut frame, &mut locals, expr) {
                            Ok(value) => Some(value),
                            Err(stop) => return Outcome::Blocked(stop.reason()),
                        },
                        None => None,
                    };
                    let called = call_of(subject);
                    let outcome = eval(&mut scope, &mut frame, &mut locals, &called);
                    let raised = match outcome {
                        Ok(_) => None,
                        // A Ruby exception that no `rescue` wanted: exactly what
                        // this matcher exists to see.
                        Err(Stop::Stopped(interp::Error::Uncaught { class, .. })) => Some(class),
                        // Anything else is a gap in Spinel, not an answer. Reported
                        // blocked, never as a raise the example caught — which is
                        // why `interp::Error` keeps the two apart at all.
                        Err(stop) => return Outcome::Blocked(stop.reason()),
                    };
                    // A `NameError` about a constant this heap has never seen is
                    // almost always a fixture file the corpus `require`s and this
                    // harness cannot load — `ModuleSpecs`, `ClassSpecs` — rather
                    // than the behaviour under test. Ruby would have the constant,
                    // so the example did not run at all, and calling that a
                    // disagreement would be inventing a failure out of a gap.
                    // It stays a *failure* when the example asked for a `NameError`,
                    // because then the raise is the thing being checked and
                    // `scripts/verify-passes.rb` re-runs it on CRuby either way.
                    if let Some(class) = &raised
                        && class == "NameError"
                        && !matches!(&wanted, Some(value) if raised_matches(&mut scope, class, *value)
                            .unwrap_or(false))
                    {
                        return Outcome::Blocked(
                            "a constant this heap has never seen; the corpus requires a fixture file"
                                .to_owned(),
                        );
                    }
                    let held = match (&raised, negated) {
                        (None, true) => true,
                        (None, false) => false,
                        (Some(_), true) => false,
                        (Some(class), false) => match wanted {
                            Some(wanted) => match raised_matches(&mut scope, class, wanted) {
                                Some(matched) => matched,
                                // The raised class is not reachable by name, so
                                // whether it is a subclass cannot be decided.
                                None => {
                                    return Outcome::Blocked(format!(
                                        "cannot tell whether {class} is the class this expects"
                                    ));
                                }
                            },
                            // `should.raise` with no class: anything counts.
                            None => true,
                        },
                    };
                    if !held {
                        let wanted = wanted.map_or_else(
                            || "an exception".to_owned(),
                            |value| interp::inspect(&mut scope, value),
                        );
                        return Outcome::Failed(match (&raised, negated) {
                            (Some(class), true) => {
                                format!("should not have raised, but raised {class}")
                            }
                            (Some(class), false) => {
                                format!("should raise {wanted}, raised {class}")
                            }
                            (None, _) => format!("should raise {wanted}, raised nothing"),
                        });
                    }
                    ran_something = true;
                }

                Statement::Effect(expr) => {
                    if let Err(stop) = eval(&mut scope, &mut frame, &mut locals, expr) {
                        return Outcome::Blocked(stop.reason());
                    }
                    ran_something = true;
                }
            }
        }

        if ran_something || example.body.is_empty() {
            Outcome::Passed
        } else {
            Outcome::Blocked("nothing to run".to_owned())
        }
    })();
    match (outcome, scope.missing_method()) {
        (Outcome::Failed(why), Some(missing)) => Outcome::Blocked(format!(
            "{missing}; the failure that followed is not a disagreement ({why})"
        )),
        (outcome, _) => outcome,
    }
}

/// Compile one expression against the shared slot map and run it in the shared
/// frame. `Err` is always a reason the example is blocked, never a failure:
/// a construct Spinel does not implement is not a disagreement with Ruby.
fn eval(
    scope: &mut HandleScope<'_>,
    frame: &mut interp::Frame,
    locals: &mut Vec<Name>,
    expr: &Expr,
) -> Result<Value, Stop> {
    let iseq = compile::expression("<example>", locals, expr)
        .map_err(|e| Stop::Unsupported(e.to_string()))?;
    // The compiler appends any local this expression introduced, and the next
    // one must see the same indices.
    locals.clone_from(&iseq.locals);
    interp::eval_in(scope, frame, &iseq).map_err(Stop::Stopped)
}

/// mspec's `ScratchPad`, which the corpus reaches for more than any other
/// helper: 427 examples in `language/` alone block without it, because
/// `rescue_spec.rb` records into it from a `before :each` hook that runs before
/// every one of its examples.
///
/// It is built here rather than in `spinel-vm` for the reason the module docs
/// give: the VM knows nothing about `should`, and it must not learn. What the VM
/// contributes is generic — [`Native::Getter`] and [`Native::Setter`], which are
/// what `attr_accessor` becomes when `core/*.rb` asks for one — and the two
/// methods that need more than a slot are written in Ruby, right here.
///
/// # The one place it is not mspec's
///
/// mspec's `<<` mutates the recorded array in place. Spinel has no growable
/// `Array` yet — that is [#15](https://github.com/ar4mirez/spinel/issues/15) —
/// so this one allocates: `record(recorded + [value])`. The difference is
/// observable by holding on to what `recorded` answered *before* an append, and
/// no example in the corpus does. That is not an argument, it is a claim, and
/// `scripts/verify-passes.rb` re-runs every example this harness passes against
/// the real `ScratchPad` on CRuby, where an example that did alias it fails
/// loudly rather than passing here.
fn install_scratch_pad(scope: &mut HandleScope<'_>, frame: &mut interp::Frame) {
    // A class with one instance, rather than mspec's class-with-class-methods:
    // a class object's slots belong to `Classes`, and an instance's are ours.
    // Nothing in the corpus asks `ScratchPad` what its class is.
    let class = scope.define_class(Some("ScratchPadRecorder"), Some(Builtin::Object.id()));
    for (name, native) in [
        ("record", Native::Setter(0)),
        ("recorded", Native::Getter(0)),
    ] {
        let body = scope.definitions_mut().add(Definition::Native(native));
        let symbol = spinel_vm::shared::symbols::intern(name);
        scope.classes_mut().define_method(class, symbol, body);
    }

    let class_object = scope.classes().object(class);
    let class_handle = scope.root(class_object);
    let handle = scope.alloc(Some(class_handle), Payload::Slots, 1);
    scope.set_slot(handle, 0, Value::NIL);
    let recorder = scope.get(handle);
    let symbol = spinel_vm::shared::symbols::intern("ScratchPad");
    scope
        .classes_mut()
        .const_set(Builtin::Object.id(), symbol, recorder);

    // The rest is Ruby, on the two primitives above.
    const PRELUDE: &str = "def ScratchPad.<<(value); record(recorded + [value]); end
def ScratchPad.clear; record(nil); end
def ScratchPad.record(value); end
";
    let parsed = spinel_parse::parse(PRELUDE.as_bytes());
    debug_assert!(
        parsed.errors.is_empty(),
        "the ScratchPad prelude must parse"
    );
    let statements: Vec<Expr> = parsed.program.body.clone();
    // `record` is already a method on the class; the singleton `def` above is
    // dropped, and only `<<` and `clear` are wanted. Compiling all three and
    // running the first two is simpler than splitting the source.
    for statement in statements.iter().take(2) {
        let Ok(iseq) = compile::expression("<scratch-pad>", &[], statement) else {
            debug_assert!(false, "the ScratchPad prelude must compile");
            return;
        };
        let ran = interp::eval_in(scope, frame, &iseq);
        debug_assert!(ran.is_ok(), "the ScratchPad prelude must run: {ran:?}");
    }
}

/// The name of the class `value` is, when it is a class.
fn class_id_of_value(scope: &mut HandleScope<'_>, value: Value) -> Option<ClassId> {
    let handle = scope.root(value);
    scope.class_id_of(handle)
}

/// Whether an exception of the class named `raised` is one the example's
/// `expected` class would catch — subclasses included, as `rescue` does.
fn raised_matches(scope: &mut HandleScope<'_>, raised: &str, expected: Value) -> Option<bool> {
    let wanted = class_id_of_value(scope, expected)?;
    let symbol = spinel_vm::shared::symbols::intern(raised);
    let object = scope
        .classes()
        .const_get_here(Builtin::Object.id(), symbol)?;
    let actual = class_id_of_value(scope, object)?;
    Some(scope.classes().ancestors(actual).contains(&wanted))
}
