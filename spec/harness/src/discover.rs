//! ruby/spec's DSL, read out of `spinel_ast`.
//!
//! ruby/spec is Ruby, so the real answer is mspec running on Spinel. Until the
//! VM exists there is no `eval`, and the only thing that can be known about a
//! spec file is what its syntax tree says. That turns out to be most of what a
//! runner needs: which examples exist, what they are called, and which guards
//! stand between them and the interpreter.
//!
//! What this module does *not* do is decide whether an example passes. That
//! needs a VM, and is [`crate::Outcome::Blocked`] until phase 1.

use spinel_ast::{BlockArg, Call, Expr, ExprKind, Program, Span, StrPart};

// ---------------------------------------------------------------------------
// The target being specced
// ---------------------------------------------------------------------------

/// What the guards are asked about: the language version Spinel implements and
/// the platform it is running on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `RUBY_VERSION`, as `ruby_version_is` compares it.
    pub language_version: Version,
    /// mspec's name for the host OS: `darwin`, `linux`, `windows`.
    pub platform: String,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            language_version: Version::parse(spinel_vm::LANGUAGE_VERSION),
            platform: host_platform().to_owned(),
        }
    }
}

/// mspec spells the host OS the way Ruby's `RUBY_PLATFORM` does, not the way
/// Rust does: macOS is `darwin`.
fn host_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

/// A dotted version, compared segment by segment.
///
/// `"3.5"` and `"3.5.0"` are the same version: a missing segment is zero, which
/// is what makes `ruby_version_is "3.5"` true on 3.5.0. Equality is defined by
/// [`Ord`] for that reason and not derived — a derived `PartialEq` would compare
/// the segment lists, and then `"3.5" == "3.5.0"` would be false while
/// `"3.5" >= "3.5.0"` was true.
#[derive(Debug, Clone)]
pub struct Version(Vec<u32>);

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Version {}

impl Version {
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self(text.split('.').filter_map(|s| s.parse().ok()).collect())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let width = self.0.len().max(other.0.len());
        let at = |v: &[u32], i: usize| v.get(i).copied().unwrap_or(0);
        (0..width)
            .map(|i| at(&self.0, i).cmp(&at(&other.0, i)))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

// ---------------------------------------------------------------------------
// Examples
// ---------------------------------------------------------------------------

/// One `it "..." do ... end`, with the `describe` strings enclosing it.
//
// Not `Eq`: the body holds float literals. Nothing compares examples.
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    /// `describe` descriptions, outermost first.
    pub group: Vec<String>,
    /// The example's own description.
    pub description: String,
    /// Where it is, for a `path:line` in the report.
    pub span: Span,
    /// The block body, so it can be run. Cloned rather than borrowed: the tree
    /// is dropped once a file's examples are collected, and an example outlives
    /// it.
    pub body: Vec<Expr>,
    /// The locals the parser assigned to the block's own scope, in slot order.
    pub locals: Vec<spinel_ast::Name>,
    /// Set when a guard excluded this example, or when the harness could not
    /// evaluate the guard and refused to guess.
    pub skipped: Option<String>,
    /// Source spans of the `before` bodies prepended to `body`, outermost first.
    ///
    /// `scripts/verify-passes.rb` re-runs a passing example on real Ruby by
    /// slicing it back out of the file, so it has to slice the same statements
    /// that ran. Without these it would eval an example whose helper method was
    /// defined in a hook it never saw, and report a false pass that is really a
    /// disagreement between the harness and its own verifier.
    pub setup_spans: Vec<Span>,
}

impl Example {
    /// The full name mspec prints and tags files key on: every enclosing
    /// `describe` and then the example, joined by spaces.
    #[must_use]
    pub fn full_description(&self) -> String {
        let mut out = self.group.join(" ");
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&self.description);
        out
    }
}

/// Collect every example in a parsed spec file.
#[must_use]
pub fn examples(program: &Program, target: &Target) -> Vec<Example> {
    let mut walk = Walk {
        target,
        group: Vec::new(),
        skipped: None,
        setup: Vec::new(),
        out: Vec::new(),
    };
    // A file's top level is a group like any other: `before` can live there.
    walk.collect_setup(&program.body);
    walk.body(&program.body);
    walk.out
}

struct Walk<'a> {
    target: &'a Target,
    group: Vec<String>,
    /// Reason the enclosing guard excluded everything below, if any.
    skipped: Option<String>,
    /// `before` bodies from the enclosing groups, outermost first. Prepended to
    /// every example in the group.
    ///
    /// `block_spec.rb` defines the method under test in one of these, so a
    /// harness that walks past them can run almost none of that file:
    ///
    /// ```ruby
    /// before :all do
    ///   def m(a) yield a end
    /// end
    /// ```
    ///
    /// `before :all` is prepended per example rather than run once, which is
    /// the only shape a fresh heap per example allows. An example that depended
    /// on state accumulated across examples will *fail* rather than falsely
    /// pass, which is the direction this harness errs in everywhere else too.
    setup: Vec<Setup>,
    out: Vec<Example>,
}

/// One `before` block's body, the locals its own scope declared, and where in
/// the file it came from.
#[derive(Clone)]
struct Setup {
    body: Vec<Expr>,
    locals: Vec<spinel_ast::Name>,
    span: Span,
}

impl Walk<'_> {
    /// Examples are found at statement level: a `describe` body is a list of
    /// statements, and `it` is one of them.
    //
    // ponytail: statement-level only. An `it` buried inside an `if` or a `case`
    // in a describe body is not found. Nothing in ruby/spec does that today —
    // `scripts/spec.sh` over the whole corpus is the check — and finding them
    // would mean a full expression visitor for no examples gained. If a count
    // ever looks short, that is the first place to look.
    fn body(&mut self, statements: &[Expr]) {
        for statement in statements {
            if let ExprKind::Call(call) = &statement.kind {
                self.call(statement.span, call);
            }
        }
    }

    fn call(&mut self, span: Span, call: &Call) {
        // `describe`, `it` and the guards are bare calls on the spec's own
        // self. A call with a receiver is ordinary Ruby — but it may still be
        // `[1, 2].each do ... it ... end`, so its block is walked.
        let is_dsl = call.receiver.is_none();

        let Some(block) = block_body(call) else {
            // `it "is pending"` with no block is mspec's pending marker.
            if is_dsl && &*call.name == "it" {
                self.push(span, call, Some("pending: no block".to_owned()));
            }
            return;
        };
        if !is_dsl {
            self.body(block);
            return;
        }

        match &*call.name {
            "describe" | "context" => {
                self.group.push(argument_text(call).unwrap_or_default());
                // Hooks first, whatever order they appear in: mspec runs a
                // group's `before` for every example in it, including the ones
                // written above the hook.
                let depth = self.setup.len();
                self.collect_setup(block);
                self.body(block);
                self.setup.truncate(depth);
                self.group.pop();
            }
            "it" | "specify" => self.push(span, call, self.skipped.clone()),
            name if is_guard(name) => {
                let outer = self.skipped.clone();
                // A guard that excludes its body still has to walk it: the
                // examples inside are skipped, not absent, and a count that
                // moved with the host platform would be a useless progress bar.
                self.skipped = outer.clone().or(match guard(name, call, self.target) {
                    Guard::Run => None,
                    Guard::Skip(reason) | Guard::Undecidable(reason) => Some(reason),
                });
                self.nested(block);
                self.skipped = outer;
            }
            // Some specs build examples in a loop. Descending into any other
            // block finds those.
            _ => self.nested(block),
        }
    }

    /// Walk a block that is not a `describe` but can still hold both hooks and
    /// examples.
    ///
    /// `struct_group_spec.rb` puts its `before :all` inside `platform_is_not`,
    /// so a walk that only collected hooks from `describe` bodies handed the
    /// examples below it a `@g` that was never assigned. They then *ran*
    /// against `nil` instead of blocking — `(@g == nil).should == false`
    /// failed, and would have as happily passed. A hook that is skipped has to
    /// be skipped visibly.
    fn nested(&mut self, statements: &[Expr]) {
        let depth = self.setup.len();
        self.collect_setup(statements);
        self.body(statements);
        self.setup.truncate(depth);
    }

    /// Find this group's `before` blocks and stack them for its examples.
    fn collect_setup(&mut self, statements: &[Expr]) {
        for statement in statements {
            let ExprKind::Call(call) = &statement.kind else {
                continue;
            };
            if call.receiver.is_some() || &*call.name != "before" {
                continue;
            }
            if let Some(BlockArg::Block(block)) = call.block.as_ref()
                && let (Some(first), Some(last)) = (block.body.first(), block.body.last())
            {
                self.setup.push(Setup {
                    span: Span {
                        start: first.span.start,
                        end: last.span.end,
                    },
                    body: block.body.clone(),
                    locals: block.locals.clone(),
                });
            }
        }
    }

    fn push(&mut self, span: Span, call: &Call, skipped: Option<String>) {
        let (own_body, own_locals) = match call.block.as_ref() {
            Some(BlockArg::Block(block)) => (block.body.clone(), block.locals.clone()),
            _ => (Vec::new(), Vec::new()),
        };
        // The hooks run first, outermost group first, then the example.
        let mut body: Vec<Expr> = Vec::new();
        let mut locals: Vec<spinel_ast::Name> = Vec::new();
        for setup in &self.setup {
            body.extend(setup.body.iter().cloned());
            for name in &setup.locals {
                if !locals.contains(name) {
                    locals.push(name.clone());
                }
            }
        }
        let mut setup_spans: Vec<Span> = self.setup.iter().map(|s| s.span).collect();
        // An example with no body of its own asserts nothing, and mspec passes
        // it. Prepending hooks must not turn that into something that ran.
        if own_body.is_empty() {
            body.clear();
            setup_spans.clear();
        } else {
            body.extend(own_body);
        }
        for name in own_locals {
            if !locals.contains(&name) {
                locals.push(name);
            }
        }
        self.out.push(Example {
            group: self.group.clone(),
            description: argument_text(call).unwrap_or_else(|| "<no description>".to_owned()),
            span,
            setup_spans,
            body,
            locals,
            skipped,
        });
    }
}

/// The body of a literal `do end` or `{ }` block, if the call has one.
fn block_body(call: &Call) -> Option<&[Expr]> {
    match call.block.as_ref()? {
        BlockArg::Block(block) => Some(&block.body),
        // `&blk` — the block is a value from elsewhere, so there is no body here.
        BlockArg::Pass(_) => None,
    }
}

/// A call's first argument as text, for `describe "..."` and `it "..."`.
///
/// Interpolation is kept as `#{}` rather than dropped: an example named
/// `"raises #{exc}"` is still an example, and losing it would undercount.
fn argument_text(call: &Call) -> Option<String> {
    let parts = match &call.args.first()?.kind {
        ExprKind::Str(s) | ExprKind::Sym(s) => &s.parts,
        // `describe Array do` names a class rather than a string.
        ExprKind::Var(spinel_ast::VarRef::Const(name)) => return Some(name.to_string()),
        _ => return None,
    };
    let mut out = String::new();
    for part in parts {
        match part {
            StrPart::Bytes(bytes) => out.push_str(&String::from_utf8_lossy(bytes)),
            StrPart::Interp(_) => out.push_str("#{}"),
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// What a guard says about the examples inside it.
enum Guard {
    /// The examples below should run.
    Run,
    /// The guard evaluated, and excludes them.
    Skip(String),
    /// The harness cannot evaluate this guard. The examples below are skipped
    /// rather than guessed at in either direction: a guard silently assumed
    /// true is a spec that reports a result it never earned.
    Undecidable(String),
}

/// Guards `spec/harness` recognises by name. Names it does not know are not
/// guards at all, and their blocks are walked normally.
//
// The two the roadmap names are evaluated. The rest are recognised only so that
// their examples are reported skipped-undecidable instead of run: `guard -> {}`
// takes a Ruby lambda, `ruby_bug` needs a CRuby bug list, and the others need
// runtime feature detection. All of them evaluate for real under mspec in
// phase 2, which is why none of them is worth half-implementing now.
fn is_guard(name: &str) -> bool {
    matches!(
        name,
        "ruby_version_is"
            | "platform_is"
            | "platform_is_not"
            | "guard"
            | "guard_not"
            | "ruby_bug"
            | "not_supported_on"
            | "not_compliant_on"
            | "with_feature"
            | "conflicts_with"
            | "quarantine!"
    )
}

fn guard(name: &str, call: &Call, target: &Target) -> Guard {
    match name {
        "ruby_version_is" => version_guard(call, target),
        "platform_is" => platform_guard(call, target, true),
        "platform_is_not" => platform_guard(call, target, false),
        other => Guard::Undecidable(format!("guard `{other}` needs a VM")),
    }
}

/// `ruby_version_is "3.5"`, `ruby_version_is ""..."3.5"`, `"3.0".."3.5"`.
///
/// A bare string is a floor. A range is a window, and its end is exclusive or
/// not exactly as the Ruby range is. An empty string is version zero, which is
/// how ruby/spec spells "everything below this".
fn version_guard(call: &Call, target: &Target) -> Guard {
    let version = &target.language_version;
    let Some(argument) = call.args.first() else {
        return Guard::Undecidable("ruby_version_is with no version".to_owned());
    };

    let bound = |expr: &Expr| -> Option<Version> {
        let text = literal_string(expr)?;
        Some(Version::parse(&text))
    };

    let (low, high, exclusive) = match &argument.kind {
        ExprKind::Range(range) => {
            let low = match &range.left {
                Some(expr) => Some(bound(expr)),
                None => Some(None),
            };
            let (Some(low), Some(high)) = (low, range.right.as_ref().map(bound)) else {
                return Guard::Undecidable("ruby_version_is with a computed bound".to_owned());
            };
            (low, high, range.exclude_end)
        }
        _ => match bound(argument) {
            Some(low) => (Some(low), None, false),
            None => return Guard::Undecidable("ruby_version_is with a computed bound".to_owned()),
        },
    };

    // `""` parses to no segments; treat it as the zero floor it means.
    let above = low
        .filter(|v| !v.is_empty())
        .is_none_or(|low| *version >= low);
    let below = high.filter(|v| !v.is_empty()).is_none_or(|high| {
        if exclusive {
            *version < high
        } else {
            *version <= high
        }
    });

    if above && below {
        Guard::Run
    } else {
        Guard::Skip(format!(
            "ruby_version_is: not ruby {}",
            spinel_vm::LANGUAGE_VERSION
        ))
    }
}

/// `platform_is :darwin`, `platform_is_not :windows`.
///
/// Only the symbol form is evaluated. `platform_is wordsize: 64` and friends
/// describe the host in ways this harness has no business guessing at.
fn platform_guard(call: &Call, target: &Target, want_match: bool) -> Guard {
    let mut names = Vec::new();
    for argument in &call.args {
        match &argument.kind {
            ExprKind::Sym(_) => match literal_string(argument) {
                Some(name) => names.push(name),
                None => {
                    return Guard::Undecidable("platform guard with a computed name".to_owned());
                }
            },
            // A hash argument is `wordsize:`, `pointer_size:`, `os:`.
            _ => return Guard::Undecidable("platform guard on a host property".to_owned()),
        }
    }
    if names.is_empty() {
        return Guard::Undecidable("platform guard with no platform".to_owned());
    }

    // mspec matches a platform name as a prefix of `RUBY_PLATFORM`, so `:darwin`
    // matches `arm64-darwin24`. Comparing OS names directly is the same test
    // without the version noise.
    let matched = names.iter().any(|name| name == &target.platform);
    if matched == want_match {
        Guard::Run
    } else {
        Guard::Skip(format!("platform is {}", target.platform))
    }
}

/// A string or symbol literal with no interpolation, as text.
fn literal_string(expr: &Expr) -> Option<String> {
    let parts = match &expr.kind {
        ExprKind::Str(s) | ExprKind::Sym(s) => &s.parts,
        _ => return None,
    };
    match parts.as_slice() {
        [] => Some(String::new()),
        [StrPart::Bytes(bytes)] => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Program {
        let parsed = spinel_parse::parse(source.as_bytes());
        assert!(
            parsed.is_ok(),
            "test fixture must parse: {:?}",
            parsed.errors
        );
        parsed.program
    }

    fn target(version: &str, platform: &str) -> Target {
        Target {
            language_version: Version::parse(version),
            platform: platform.to_owned(),
        }
    }

    fn names(source: &str, target: &Target) -> Vec<String> {
        examples(&parse(source), target)
            .iter()
            .map(Example::full_description)
            .collect()
    }

    fn skips(source: &str, target: &Target) -> Vec<Option<String>> {
        examples(&parse(source), target)
            .into_iter()
            .map(|e| e.skipped)
            .collect()
    }

    #[test]
    fn version_segments_compare_numerically_not_as_text() {
        // The bug this catches: "4.10" sorting before "4.9" as a string.
        assert!(Version::parse("4.10") > Version::parse("4.9"));
        assert_eq!(Version::parse("3.5"), Version::parse("3.5.0"));
        assert!(Version::parse("4.0.0") > Version::parse("3.5"));
    }

    #[test]
    fn describe_nesting_becomes_the_full_description() {
        let source = r##"
            describe "Array" do
              describe "#first" do
                it "returns the first element" do
                end
              end
              it "is a class" do
              end
            end
        "##;
        assert_eq!(
            names(source, &target("4.0.0", "darwin")),
            ["Array #first returns the first element", "Array is a class"]
        );
    }

    #[test]
    fn a_version_floor_admits_the_target_and_a_ceiling_excludes_it() {
        let source = r##"
            ruby_version_is "3.5" do
              it "runs on new rubies" do; end
            end
            ruby_version_is ""..."3.5" do
              it "runs on old rubies" do; end
            end
        "##;
        assert_eq!(
            skips(source, &target("4.0.0", "darwin")),
            [None, Some("ruby_version_is: not ruby 4.0.0".to_owned())]
        );
    }

    #[test]
    fn an_inclusive_range_end_admits_its_own_version() {
        let source = r##"
            ruby_version_is "3.0".."4.0" do
              it "included" do; end
            end
            ruby_version_is "3.0"..."4.0" do
              it "excluded" do; end
            end
        "##;
        let skipped = skips(source, &target("4.0.0", "darwin"));
        assert_eq!(skipped[0], None);
        assert!(skipped[1].is_some());
    }

    #[test]
    fn platform_guards_read_the_host_both_ways() {
        let source = r##"
            platform_is :darwin do
              it "on a mac" do; end
            end
            platform_is_not :darwin do
              it "not on a mac" do; end
            end
            platform_is :linux, :darwin do
              it "on either" do; end
            end
        "##;
        let skipped = skips(source, &target("4.0.0", "darwin"));
        assert_eq!(skipped[0], None);
        assert_eq!(skipped[1], Some("platform is darwin".to_owned()));
        assert_eq!(skipped[2], None);
    }

    #[test]
    fn a_guard_the_harness_cannot_evaluate_skips_rather_than_guesses() {
        // The failure this prevents: assuming the guard is true, running the
        // examples, and reporting a result the harness never earned.
        let source = r##"
            guard -> { Kernel.respond_to?(:fork) } do
              it "forks" do; end
            end
            platform_is wordsize: 64 do
              it "is 64-bit" do; end
            end
        "##;
        for skipped in skips(source, &target("4.0.0", "darwin")) {
            assert!(skipped.is_some(), "an undecidable guard must skip");
        }
    }

    #[test]
    fn an_excluded_guard_still_reports_the_examples_it_hides() {
        // A count that shrank on a different host would be a useless progress bar.
        let source = r##"
            platform_is :windows do
              describe "Windows" do
                it "one" do; end
                it "two" do; end
              end
            end
        "##;
        assert_eq!(names(source, &target("4.0.0", "darwin")).len(), 2);
    }

    #[test]
    fn examples_built_in_a_loop_are_found() {
        let source = r##"
            describe "each" do
              [1, 2].each do |n|
                it "handles #{n}" do; end
              end
            end
        "##;
        assert_eq!(
            names(source, &target("4.0.0", "darwin")),
            ["each handles #{}"]
        );
    }

    #[test]
    fn a_pending_example_has_no_block_and_is_still_counted() {
        let source = r##"
            describe "pending" do
              it "is not written yet"
            end
        "##;
        assert_eq!(
            skips(source, &target("4.0.0", "darwin")),
            [Some("pending: no block".to_owned())]
        );
    }

    #[test]
    fn should_is_not_mistaken_for_the_dsl() {
        // `it` and `describe` are bare calls. A method of the same name on a
        // receiver is ordinary Ruby and must not create an example.
        let source = r##"
            describe "x" do
              it "one" do
                subject.it "not an example"
                other.describe "not a group"
              end
            end
        "##;
        assert_eq!(names(source, &target("4.0.0", "darwin")), ["x one"]);
    }
}
