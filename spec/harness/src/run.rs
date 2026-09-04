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

use spinel_ast::{Expr, ExprKind, Name};
use spinel_vm::{Heap, compile, interp};

use crate::discover::Example;

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
    /// Anything else: run it for its effect and discard the value.
    Effect(&'a Expr),
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
    Statement::Effect(expr)
}

/// Compile and run one example.
///
/// A fresh [`Heap`] per example, because examples must not see each other's
/// objects and there is no other isolation yet. Bootstrapping one is a few
/// hundred allocations; the whole `language/` corpus runs in well under a
/// second, so the simple thing is also the fast enough thing.
pub fn run(example: &Example) -> Outcome {
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

    // One slot map for the whole example: every statement is compiled against
    // it and hands back the version it grew, so `a` is the same slot in the
    // statement that writes it and the one that reads it.
    let mut locals: Vec<Name> = example.locals.clone();
    for name in compile::declared_locals(&example.body) {
        if !locals.contains(&name) {
            locals.push(name);
        }
    }

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
                    Err(blocked) => return Outcome::Blocked(blocked),
                };
                let wanted = match eval(&mut scope, &mut frame, &mut locals, expected) {
                    Ok(value) => value,
                    Err(blocked) => return Outcome::Blocked(blocked),
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
            Statement::Effect(expr) => {
                if let Err(blocked) = eval(&mut scope, &mut frame, &mut locals, expr) {
                    return Outcome::Blocked(blocked);
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
}

/// Compile one expression against the shared slot map and run it in the shared
/// frame. `Err` is always a reason the example is blocked, never a failure:
/// a construct Spinel does not implement is not a disagreement with Ruby.
fn eval(
    scope: &mut spinel_vm::HandleScope<'_>,
    frame: &mut interp::Frame,
    locals: &mut Vec<Name>,
    expr: &Expr,
) -> Result<spinel_vm::Value, String> {
    let iseq = compile::expression("<example>", locals, expr).map_err(|e| e.to_string())?;
    // The compiler appends any local this expression introduced, and the next
    // one must see the same indices.
    locals.clone_from(&iseq.locals);
    interp::eval_in(scope, frame, &iseq).map_err(|e| e.to_string())
}
