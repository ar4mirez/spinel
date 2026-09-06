//! End-to-end tests for the `spec-harness` binary.
//!
//! Most of them build a spec file in a temporary directory, so they neither
//! need the ruby/spec submodule nor care what upstream does to it. The one that
//! does need it is [`if_spec_reports_every_example`], which is the issue's
//! definition of done and is worth pinning to the real file.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_spec-harness");

fn run(arguments: &[&str]) -> Output {
    Command::new(EXE)
        .args(arguments)
        .output()
        .expect("the harness binary should be runnable")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The repository root, from this crate's manifest directory (`spec/harness`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("spec/harness is two levels below the repository root")
        .to_path_buf()
}

fn corpus() -> PathBuf {
    let corpus = repo_root().join("spec/ruby");
    assert!(
        corpus.join("spec_helper.rb").is_file(),
        "ruby/spec is not checked out at spec/ruby.\n\
         Run: git submodule update --init spec/ruby"
    );
    corpus
}

/// A throwaway directory holding one spec file, removed when the test ends.
struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str, source: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("spinel-spec-harness-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp directory should be creatable");
        std::fs::write(dir.join(format!("{name}_spec.rb")), source)
            .expect("fixture should be writable");
        Self(dir)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("temp path should be UTF-8")
    }

    /// Write this fixture's tag file into a `tags/` directory beside the spec,
    /// and answer the arguments that run the harness against both.
    ///
    /// A spec file outside `spec/ruby` maps to its own name under the tags root,
    /// so the directory is flat: `two_spec.rb` reads `tags/two_tags.txt`.
    fn tagged(&self, name: &str, tags: &str) -> Vec<String> {
        let root = self.0.join("tags");
        std::fs::create_dir_all(&root).expect("tags directory should be creatable");
        std::fs::write(root.join(format!("{name}_tags.txt")), tags)
            .expect("tag file should be writable");
        vec![
            "--tags".to_owned(),
            root.to_str().expect("temp path should be UTF-8").to_owned(),
            self.0
                .join(format!("{name}_spec.rb"))
                .to_str()
                .expect("temp path should be UTF-8")
                .to_owned(),
        ]
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const TWO_EXAMPLES: &str = r##"
describe "Something" do
  it "works" do
    1.should == 1
  end

  it "also works" do
    2.should == 2
  end
end
"##;

#[test]
fn a_spec_file_reports_its_examples_and_succeeds() {
    let fixture = Fixture::new("two", TWO_EXAMPLES);
    let output = run(&[fixture.path()]);

    assert!(output.status.success(), "a clean run exits zero");
    let text = stdout(&output);
    assert!(
        text.contains("two_spec.rb · 2 examples · 2 passed · 0 failed · 0 blocked · 0 skipped"),
        "unexpected report:\n{text}"
    );
}

/// One example Spinel can run and one it cannot, in that order.
///
/// The unrunnable one has to keep moving as slices land: it was a method call
/// until [#11](https://github.com/ar4mirez/spinel/issues/11) compiled those,
/// and an instance variable until
/// [#151](https://github.com/ar4mirez/spinel/issues/151) gave objects a shape
/// to put one in. What it tests is not the particular construct but the rule
/// that an unimplemented one *blocks* rather than fails.
const ONE_OF_EACH: &str = r##"
describe "Something" do
  it "is runnable" do
    (1 + 1).should == 2
  end

  it "needs a class variable" do
    @@a = 2
    @@a.should == 2
  end
end
"##;

#[test]
fn an_example_it_cannot_run_is_blocked_and_never_failed() {
    // The count that must not drift. A harness that reported a pass it could
    // not have earned would make the project's progress bar a lie; one that
    // reported a *failure* for a construct Spinel simply has not written yet
    // would make the failure column useless for finding real disagreements.
    let fixture = Fixture::new("mixed", ONE_OF_EACH);
    let text = stdout(&run(&[fixture.path()]));

    assert!(
        text.contains("2 examples · 1 passed · 0 failed · 1 blocked"),
        "unsupported must block, not fail:\n{text}"
    );
    assert!(
        text.contains("blocked by, most examples first"),
        "the report must say what blocked it:\n{text}"
    );
    assert!(
        text.contains("a class variable is not compiled yet"),
        "the reason must name the construct:\n{text}"
    );
}

#[test]
fn a_disagreement_with_ruby_is_a_failure_and_fails_the_run() {
    // The other half: when Spinel *can* run an example and gets it wrong, that
    // has to be loud and has to exit non-zero.
    let fixture = Fixture::new(
        "wrong",
        "describe \"x\" do\n  it \"is wrong\" do\n    1.should == 2\n  end\nend\n",
    );
    let output = run(&[fixture.path()]);
    let text = stdout(&output);

    assert!(
        text.contains("0 passed · 1 failed"),
        "unexpected report:\n{text}"
    );
    assert!(
        text.contains("1 should equal 2"),
        "the report must say how:\n{text}"
    );
    assert!(!output.status.success(), "a failing example fails the run");
}

#[test]
fn list_prints_the_full_description_of_every_example() {
    let fixture = Fixture::new("list", TWO_EXAMPLES);
    let text = stdout(&run(&["--list", fixture.path()]));

    assert!(text.contains("Something works"), "missing example:\n{text}");
    assert!(
        text.contains("Something also works"),
        "missing example:\n{text}"
    );
}

#[test]
fn a_guard_the_harness_cannot_evaluate_skips_instead_of_running() {
    let fixture = Fixture::new(
        "guarded",
        r##"
describe "Guarded" do
  guard -> { some_runtime_check } do
    it "needs a VM to know" do
    end
  end
end
"##,
    );
    let text = stdout(&run(&[fixture.path()]));
    assert!(
        text.contains("1 skipped") && text.contains("0 blocked"),
        "an undecidable guard must skip, not run:\n{text}"
    );
}

#[test]
fn a_file_that_yields_no_examples_is_named_rather_than_counted_as_zero() {
    // Silence here would read as an empty corpus, which is the one result a
    // spec runner must never fake.
    let fixture = Fixture::new(
        "delegating",
        r##"
describe "Delegating" do
  it_behaves_like :some_shared_spec, :method
end
"##,
    );
    let text = stdout(&run(&[fixture.path()]));
    assert!(
        text.contains("no examples found (1)"),
        "an empty spec file must be named:\n{text}"
    );
}

#[test]
fn a_spec_file_that_does_not_parse_fails_the_run() {
    let fixture = Fixture::new("broken", "describe \"unclosed\" do\n  it \"x\" do\n");
    let output = run(&[fixture.path()]);

    assert!(!output.status.success(), "a broken spec file fails the run");
    assert!(
        stdout(&output).contains("could not be parsed"),
        "the report must name it:\n{}",
        stdout(&output)
    );
}

#[test]
fn a_path_with_no_specs_is_an_error_not_a_clean_run() {
    let dir = std::env::temp_dir().join("spinel-spec-harness-empty");
    std::fs::create_dir_all(&dir).expect("temp directory should be creatable");
    let output = run(&[dir.to_str().expect("temp path should be UTF-8")]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "an empty corpus is a usage error"
    );
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        text.contains("submodule update --init"),
        "the likeliest cause should be named:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_path_is_a_usage_error() {
    let output = run(&["/no/such/directory"]);
    assert_eq!(output.status.code(), Some(2));
}

// ---------------------------------------------------------------------------
// `spec/tags/` — see spec/tags/README.md
// ---------------------------------------------------------------------------

/// One example Spinel gets right and one it gets wrong. Without a tag the wrong
/// one fails the run, which is what makes the tagged runs below mean something.
const ONE_WRONG: &str = r##"
describe "Tagged" do
  it "agrees" do
    1.should == 1
  end

  it "disagrees" do
    1.should == 2
  end
end
"##;

fn run_owned(arguments: &[String]) -> Output {
    let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
    run(&borrowed)
}

#[test]
fn a_tagged_example_is_skipped_with_its_reason_and_never_passed() {
    let fixture = Fixture::new("tagged", ONE_WRONG);
    let arguments = fixture.tagged(
        "tagged",
        "fails(the coercion protocol, #99, closes this):Tagged disagrees\n",
    );
    let output = run_owned(&arguments);
    let text = stdout(&output);

    assert!(
        text.contains("2 examples · 1 passed · 0 failed · 0 blocked · 1 skipped"),
        "a tag must skip, never pass and never fail:\n{text}"
    );
    assert!(output.status.success(), "a tagged run is clean:\n{text}");

    // The reason has to reach the reader, or the tag is an unexplained skip.
    let listed = stdout(&run_owned(
        &[vec!["--list".to_owned()], arguments.clone()].concat(),
    ));
    assert!(
        listed.contains("skipped: the coercion protocol, #99, closes this"),
        "the reason must be reported:\n{listed}"
    );
}

#[test]
fn a_tag_without_a_reason_fails_the_run() {
    // mspec's own tag files look like this and mspec accepts them. Spinel does
    // not: a skip nobody wrote a reason for is a spec swept under the rug, and
    // the old `skip.txt` reader dropped such a line in silence.
    let fixture = Fixture::new("unreasoned", ONE_WRONG);
    let output = run_owned(&fixture.tagged("unreasoned", "fails:Tagged disagrees\n"));
    let text = stdout(&output);

    assert!(!output.status.success(), "a reasonless tag fails the run");
    assert!(
        text.contains("tag problems") && text.contains("no reason"),
        "the report must say what is wrong:\n{text}"
    );
    assert!(
        text.contains("1 failed"),
        "and the example must not be skipped by it:\n{text}"
    );
}

#[test]
fn a_tag_naming_no_example_fails_the_run() {
    // The rot this exists to catch: upstream rewords one `it` and the tag goes
    // on skipping nothing, in silence, forever.
    let fixture = Fixture::new("stale", ONE_WRONG);
    let output = run_owned(&fixture.tagged(
        "stale",
        "fails(a reason):Tagged disagreed under its old name\n",
    ));
    let text = stdout(&output);

    assert!(!output.status.success(), "a stale tag fails the run");
    assert!(
        text.contains("no example named `Tagged disagreed under its old name`"),
        "the report must name the dead tag:\n{text}"
    );
}

#[test]
fn a_reason_holding_a_parenthesis_fails_the_run() {
    // mspec's tag parser closes the reason at the first `)`, fails to match the
    // rest of the line, and drops the tag without a word. Loud here instead of
    // silent after #145.
    let fixture = Fixture::new("parens", ONE_WRONG);
    let output = run_owned(&fixture.tagged(
        "parens",
        "fails(m(*a) must expand first):Tagged disagrees\n",
    ));
    let text = stdout(&output);

    assert!(!output.status.success(), "a parenthesis fails the run");
    assert!(
        text.contains("parenthesis"),
        "the report must say why:\n{text}"
    );
}

#[test]
fn an_unknown_tag_fails_the_run() {
    let fixture = Fixture::new("unknown", ONE_WRONG);
    let output = run_owned(&fixture.tagged("unknown", "slow(takes a while):Tagged disagrees\n"));

    assert!(
        !output.status.success(),
        "a tag the harness ignores would look live and do nothing"
    );
    assert!(stdout(&output).contains("unknown tag `slow`"));
}

#[test]
fn a_tag_problem_alone_fails_a_run_with_nothing_else_wrong() {
    // The other tag tests all use a spec file with a disagreeing example in it,
    // so their non-zero exit is also explained by `1 failed` — which means none
    // of them can prove the exit code is wired to tag problems at all. Verified
    // by mutation: deleting `!tag_problems.is_empty()` from the exit condition
    // left every one of them green, and only this test red.
    let fixture = Fixture::new("cleanbutstale", TWO_EXAMPLES);
    let output = run_owned(&fixture.tagged(
        "cleanbutstale",
        "fails(a reason):Something that was renamed upstream\n",
    ));
    let text = stdout(&output);

    assert!(
        text.contains("2 passed · 0 failed"),
        "nothing but the tag is wrong here:\n{text}"
    );
    assert!(
        !output.status.success(),
        "a tag problem must fail the run on its own:\n{text}"
    );
}

#[test]
fn a_spec_file_with_no_tag_file_is_not_a_problem() {
    // Almost no spec has a tag, and the whole point of the directory is to be
    // empty one day.
    let fixture = Fixture::new("untagged", TWO_EXAMPLES);
    let output = run(&[fixture.path()]);

    assert!(output.status.success());
    assert!(!stdout(&output).contains("tag problems"));
}

// ---------------------------------------------------------------------------
// Against the real corpus
// ---------------------------------------------------------------------------

#[test]
fn if_spec_reports_every_example() {
    // 52 is the number of `it` blocks in the file at the pinned commit; if
    // upstream edits the file this test is where that shows up, which is the
    // point of pinning a submodule.
    //
    // The pass count is a *ratchet*, not an equality: this slice earned 27 and
    // the ones after it earn more, so an exact number would have to be edited
    // by every author who improved things and would eventually be edited
    // downwards by one who did not notice. What must never move is `0 failed`.
    const EARNED: usize = 27;
    let path = corpus().join("language/if_spec.rb");
    let output = run(&[path.to_str().expect("corpus path should be UTF-8")]);

    assert!(output.status.success(), "the run itself must succeed");
    let text = stdout(&output);
    assert!(text.contains("52 examples"), "unexpected report:\n{text}");
    assert!(
        text.contains("0 failed"),
        "no example may disagree with Ruby:\n{text}"
    );

    let passed = passed_count(&text).expect("the report should carry a pass count");
    assert!(
        passed >= EARNED,
        "if_spec.rb passed {passed}, down from the {EARNED} this slice earned:\n{text}"
    );
}

/// The `N passed` out of a report line.
fn passed_count(text: &str) -> Option<usize> {
    let (before, _) = text.split_once(" passed")?;
    before.rsplit(' ').next()?.parse().ok()
}

#[test]
fn the_whole_corpus_parses_and_reports() {
    // A floor, not an exact count: upstream adds specs, and this test exists to
    // catch the corpus going missing or the parser regressing, not to be
    // rewritten every time ruby/spec grows. The exact number lives in the PRD.
    let corpus = corpus();
    let output = run(&[corpus.to_str().expect("corpus path should be UTF-8")]);
    let text = stdout(&output);

    // Also the only full-corpus check that every `spec/tags/` file still points
    // at a live example: the harness only reads the tag file of a spec file it
    // runs, so a stale tag outside `language/` surfaces here and nowhere else.
    assert!(
        output.status.success(),
        "every `*_spec.rb` in ruby/spec must parse, and every tag must name an \
         example that exists:\n{text}"
    );
    assert!(
        !text.contains("could not be parsed"),
        "a spec file Spinel cannot parse is a parser bug:\n{text}"
    );

    let examples = text
        .split_once(" examples ·")
        .and_then(|(head, _)| head.rsplit(' ').next()?.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("the summary should carry an example count:\n{text}"));
    assert!(
        examples > 20_000,
        "ruby/spec has ~25k examples; found {examples}, so the corpus is not all there"
    );
}
