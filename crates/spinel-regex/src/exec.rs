//! [`Ast`] to a program, and the backtracking machine that runs it.
//!
//! Backtracking rather than a Thompson NFA because Ruby's dialect has
//! backreferences and lookaround, neither of which a finite automaton can
//! express. The machine is iterative over an explicit stack for the main flow,
//! and recursive only for lookaround and atomic groups, whose nesting depth is
//! a property of the pattern rather than of the subject.

use crate::Error;
use crate::parse::{Anchor, Ast, Class, ClassItem, Perl, Posix};

#[derive(Debug, Clone)]
enum Inst {
    Char {
        ch: char,
        icase: bool,
    },
    Class(u32),
    Any {
        nl: bool,
    },
    /// Try `prefer` first; on failure resume at `alt`.
    Split {
        prefer: usize,
        alt: usize,
    },
    Jump(usize),
    Save(usize),
    Anchor(Anchor),
    Backref {
        group: usize,
        icase: bool,
    },
    /// Run the sub-program at `start` without consuming input.
    Look {
        behind: bool,
        negate: bool,
        start: usize,
    },
    /// Run the sub-program at `start`, keep its first match, discard its
    /// alternatives. `(?>...)` and every possessive quantifier.
    Atomic {
        start: usize,
    },
    /// Record the state at the top of a loop iteration: position, captures,
    /// and how deep the backtrack stack was.
    Mark(usize),
    /// Onigmo's empty check, at the bottom of a loop iteration.
    ///
    /// An iteration that consumed nothing may still go round again, so long as
    /// it *changed a capture* — that is what lets `/(a|\2b|())*/` cross the
    /// empty iteration in the middle of `"aaabbb"` and carry on matching. When
    /// nothing moved and nothing changed, the loop stops, and it stops with a
    /// cut: the body's remaining alternatives are discarded rather than
    /// retried, which is why `/(?:|a)*/` matches `""` against `"aaa"` instead
    /// of backtracking into the `a`. Measured in `empty_checks_spec.rb`.
    Progress {
        slot: usize,
        exit: usize,
    },
    /// End of a sub-program.
    Accept,
    Match,
}

/// A compiled pattern.
pub struct Program {
    insts: Vec<Inst>,
    classes: Vec<Class>,
    pub groups: usize,
    marks: usize,
}

/// Where each capture landed, as byte offsets into the subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub caps: Vec<Option<(usize, usize)>>,
}

/// A pattern that expands past this many instructions is refused rather than
/// compiled. `a{1,100000}` is legal Ruby and would otherwise turn into a
/// program larger than the program that wrote it.
const MAX_PROGRAM: usize = 100_000;

/// Backtracking is exponential on patterns built to make it so. Ruby answers
/// those slowly; this engine refuses, because a spec run that hangs reports
/// nothing at all. Tuned well above anything ruby/spec needs.
const MAX_STEPS: u64 = 5_000_000;

pub fn compile(ast: &Ast, groups: usize) -> Result<Program, Error> {
    let mut c = Compiler {
        insts: Vec::new(),
        classes: Vec::new(),
        marks: 0,
    };
    c.emit(Inst::Save(0));
    c.node(ast)?;
    c.emit(Inst::Save(1));
    c.emit(Inst::Match);
    Ok(Program {
        insts: c.insts,
        classes: c.classes,
        groups,
        marks: c.marks,
    })
}

struct Compiler {
    insts: Vec<Inst>,
    classes: Vec<Class>,
    marks: usize,
}

impl Compiler {
    fn emit(&mut self, inst: Inst) -> usize {
        self.insts.push(inst);
        self.insts.len() - 1
    }

    fn here(&self) -> usize {
        self.insts.len()
    }

    fn too_large(&self) -> Result<(), Error> {
        if self.insts.len() > MAX_PROGRAM {
            return Err(Error::Syntax("pattern is too large".into()));
        }
        Ok(())
    }

    fn node(&mut self, ast: &Ast) -> Result<(), Error> {
        self.too_large()?;
        match ast {
            Ast::Empty => {}
            Ast::Literal { ch, icase } => {
                self.emit(Inst::Char {
                    ch: *ch,
                    icase: *icase,
                });
            }
            Ast::Class(class) => {
                let index = u32::try_from(self.classes.len())
                    .map_err(|_| Error::Syntax("too many character classes".into()))?;
                self.classes.push(class.clone());
                self.emit(Inst::Class(index));
            }
            Ast::Any { nl } => {
                self.emit(Inst::Any { nl: *nl });
            }
            Ast::Anchor(a) => {
                self.emit(Inst::Anchor(*a));
            }
            Ast::Backref { group, icase } => {
                self.emit(Inst::Backref {
                    group: *group,
                    icase: *icase,
                });
            }
            Ast::Concat(items) => {
                for item in items {
                    self.node(item)?;
                }
            }
            Ast::Group { index, body } => match index {
                Some(n) => {
                    self.emit(Inst::Save(n * 2));
                    self.node(body)?;
                    self.emit(Inst::Save(n * 2 + 1));
                }
                None => self.node(body)?,
            },
            Ast::Alt(branches) => self.alternation(branches)?,
            Ast::Repeat {
                body,
                min,
                max,
                greedy,
            } => self.repeat(body, *min, *max, *greedy)?,
            Ast::Atomic(body) => {
                self.sub_program(body, |start| Inst::Atomic { start })?;
            }
            Ast::Look {
                behind,
                negate,
                body,
            } => {
                let (behind, negate) = (*behind, *negate);
                self.sub_program(body, move |start| Inst::Look {
                    behind,
                    negate,
                    start,
                })?;
            }
        }
        Ok(())
    }

    /// A sub-program sits inline, right after the instruction that calls it,
    /// with a jump over it on the main path. Laying it out in this order means
    /// no address ever has to move once it is written.
    fn sub_program(&mut self, body: &Ast, make: impl FnOnce(usize) -> Inst) -> Result<(), Error> {
        let call = self.emit(Inst::Jump(0));
        let skip = self.emit(Inst::Jump(0));
        let start = self.here();
        self.node(body)?;
        self.emit(Inst::Accept);
        let after = self.here();
        self.insts[call] = make(start);
        self.insts[skip] = Inst::Jump(after);
        Ok(())
    }

    fn alternation(&mut self, branches: &[Ast]) -> Result<(), Error> {
        let Some((last, rest)) = branches.split_last() else {
            return Ok(());
        };
        let mut jumps = Vec::new();
        for branch in rest {
            let split = self.emit(Inst::Split { prefer: 0, alt: 0 });
            let prefer = self.here();
            self.node(branch)?;
            jumps.push(self.emit(Inst::Jump(0)));
            let alt = self.here();
            self.insts[split] = Inst::Split { prefer, alt };
        }
        self.node(last)?;
        let end = self.here();
        for jump in jumps {
            self.insts[jump] = Inst::Jump(end);
        }
        Ok(())
    }

    fn repeat(
        &mut self,
        body: &Ast,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    ) -> Result<(), Error> {
        // The mandatory part is written out in full.
        for _ in 0..min {
            self.node(body)?;
            self.too_large()?;
        }
        match max {
            // `{n,}` — the tail is an unbounded loop.
            None => self.star(body, greedy),
            Some(max) => {
                let optional = max.saturating_sub(min);
                if optional == 0 {
                    return Ok(());
                }
                // `{n,m}` — nested optionals, outermost first, so a greedy
                // repeat takes as many as it can.
                let mut splits = Vec::new();
                for _ in 0..optional {
                    let split = self.emit(Inst::Split { prefer: 0, alt: 0 });
                    let target = self.here();
                    splits.push((split, target));
                    self.node(body)?;
                    self.too_large()?;
                }
                let end = self.here();
                for (split, target) in splits {
                    self.insts[split] = if greedy {
                        Inst::Split {
                            prefer: target,
                            alt: end,
                        }
                    } else {
                        Inst::Split {
                            prefer: end,
                            alt: target,
                        }
                    };
                }
                Ok(())
            }
        }
    }

    /// `x*`, as a loop rather than as unrolled copies.
    fn star(&mut self, body: &Ast, greedy: bool) -> Result<(), Error> {
        let slot = self.marks;
        self.marks += 1;

        let head = self.here();
        let split = self.emit(Inst::Split { prefer: 0, alt: 0 });
        let body_start = self.here();
        self.emit(Inst::Mark(slot));
        self.node(body)?;
        let progress = self.emit(Inst::Progress { slot, exit: 0 });
        self.emit(Inst::Jump(head));
        let end = self.here();

        self.insts[split] = if greedy {
            Inst::Split {
                prefer: body_start,
                alt: end,
            }
        } else {
            Inst::Split {
                prefer: end,
                alt: body_start,
            }
        };
        self.insts[progress] = Inst::Progress { slot, exit: end };
        Ok(())
    }
}

/// The state at the top of a loop iteration, for the empty check below it.
///
/// ponytail: the captures are cloned once per iteration. Onigmo tracks only
/// the groups that live inside the loop; narrow this the same way if a real
/// pattern ever makes it the hot path.
#[derive(Clone)]
struct Mark {
    sp: usize,
    saves: Vec<Option<usize>>,
    depth: usize,
}

/// One saved point to come back to when the current path fails.
struct Backtrack {
    pc: usize,
    sp: usize,
    saves: Vec<Option<usize>>,
    marks: Vec<Option<Mark>>,
}

/// What a sub-program hands back: where it finished, and the captures it set.
type SubMatch = (usize, Vec<Option<usize>>);

impl Program {
    /// The leftmost match at or after `start`, which is what Ruby's `=~` wants.
    pub fn find_at(&self, haystack: &str, start: usize) -> Result<Option<Match>, Error> {
        let mut budget = MAX_STEPS;
        let mut at = start;
        loop {
            if let Some(m) = self.run_from(haystack, at, &mut budget)? {
                return Ok(Some(m));
            }
            if at >= haystack.len() {
                return Ok(None);
            }
            // Advance one character, never one byte: a match can only begin on
            // a character boundary.
            at += haystack[at..].chars().next().map_or(1, char::len_utf8);
        }
    }

    fn run_from(
        &self,
        haystack: &str,
        at: usize,
        budget: &mut u64,
    ) -> Result<Option<Match>, Error> {
        let saves = vec![None; (self.groups + 1) * 2];
        let marks = vec![None; self.marks];
        let Some((_, saves)) = self.run(haystack, 0, at, at, saves, marks, budget)? else {
            return Ok(None);
        };
        let caps = (0..=self.groups)
            .map(|g| match (saves[g * 2], saves[g * 2 + 1]) {
                (Some(s), Some(e)) => Some((s, e)),
                _ => None,
            })
            .collect();
        Ok(Some(Match { caps }))
    }

    /// Run from `pc` at `sp`. Answers where the first successful path finished
    /// and the captures it set, or `None` if every path fails.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        haystack: &str,
        mut pc: usize,
        mut sp: usize,
        anchor: usize,
        mut saves: Vec<Option<usize>>,
        mut marks: Vec<Option<Mark>>,
        budget: &mut u64,
    ) -> Result<Option<SubMatch>, Error> {
        let mut stack: Vec<Backtrack> = Vec::new();

        loop {
            let failed = loop {
                *budget = budget.saturating_sub(1);
                if *budget == 0 {
                    return Err(Error::Budget);
                }

                let advanced = match &self.insts[pc] {
                    Inst::Match | Inst::Accept => return Ok(Some((sp, saves))),
                    Inst::Char { ch, icase } => match next_char(haystack, sp) {
                        Some((c, width)) if chars_equal(c, *ch, *icase) => {
                            sp += width;
                            true
                        }
                        _ => false,
                    },
                    Inst::Any { nl } => match next_char(haystack, sp) {
                        Some((c, width)) if *nl || c != '\n' => {
                            sp += width;
                            true
                        }
                        _ => false,
                    },
                    Inst::Class(index) => {
                        let class = &self.classes[*index as usize];
                        match next_char(haystack, sp) {
                            Some((c, width)) if class_matches(class, c) => {
                                sp += width;
                                true
                            }
                            _ => false,
                        }
                    }
                    Inst::Anchor(a) => anchor_holds(*a, haystack, sp, anchor),
                    Inst::Save(slot) => {
                        saves[*slot] = Some(sp);
                        pc += 1;
                        continue;
                    }
                    Inst::Mark(slot) => {
                        marks[*slot] = Some(Mark {
                            sp,
                            saves: saves.clone(),
                            depth: stack.len(),
                        });
                        pc += 1;
                        continue;
                    }
                    Inst::Progress { slot, exit } => {
                        let stalled = marks[*slot]
                            .as_ref()
                            .is_some_and(|mark| mark.sp == sp && mark.saves == saves);
                        if stalled {
                            // Nothing moved and nothing changed: end the loop,
                            // and cut away the alternatives this iteration
                            // opened so none of them is retried.
                            if let Some(mark) = &marks[*slot] {
                                stack.truncate(mark.depth);
                            }
                            pc = *exit;
                        } else {
                            // The iteration completed. Onigmo does not roll a
                            // *completed* iteration's captures back when the
                            // loop later backtracks out of it — which is why
                            // `/^(()|a)*?$/ =~ "aa"` leaves `$2` as `""` and
                            // not nil, even though the path that set it was
                            // abandoned. Rolling back *within* an iteration
                            // still happens: `/((a)x|a)*/ =~ "aa"` leaves `$2`
                            // nil, because `(a)x` failed before this point was
                            // ever reached.
                            //
                            // So the commit is here, at the bottom of the
                            // iteration, and it reaches exactly the alternatives
                            // this iteration opened: everything the mark did not
                            // already see.
                            //
                            // ponytail: rewrites one capture vector per surviving
                            // alternative, so a body with many alternatives pays
                            // per iteration. Onigmo instead pushes a restore
                            // record only for the groups that need one; narrow it
                            // the same way if a real pattern makes this the hot
                            // path.
                            if let Some(mark) = &marks[*slot] {
                                for entry in stack.iter_mut().skip(mark.depth) {
                                    entry.saves.clone_from(&saves);
                                }
                            }
                            pc += 1;
                        }
                        continue;
                    }
                    Inst::Jump(target) => {
                        pc = *target;
                        continue;
                    }
                    Inst::Split { prefer, alt } => {
                        stack.push(Backtrack {
                            pc: *alt,
                            sp,
                            saves: saves.clone(),
                            marks: marks.clone(),
                        });
                        pc = *prefer;
                        continue;
                    }
                    Inst::Backref { group, icase } => {
                        match (
                            saves.get(group * 2).copied().flatten(),
                            saves.get(group * 2 + 1).copied().flatten(),
                        ) {
                            (Some(s), Some(e)) => {
                                let text = haystack[s..e].to_owned();
                                if text_at(haystack, sp, &text, *icase) {
                                    sp += text.len();
                                    true
                                } else {
                                    false
                                }
                            }
                            // A backreference to a group that never matched
                            // fails, rather than matching the empty string.
                            _ => false,
                        }
                    }
                    Inst::Atomic { start } => {
                        let inner = self.run(
                            haystack,
                            *start,
                            sp,
                            anchor,
                            saves.clone(),
                            marks.clone(),
                            budget,
                        )?;
                        match inner {
                            // Committed: the sub-match's position and captures
                            // stand, and its alternatives are gone for good.
                            Some((end, inner_saves)) => {
                                sp = end;
                                saves = inner_saves;
                                pc += 1;
                                continue;
                            }
                            None => false,
                        }
                    }
                    Inst::Look {
                        behind,
                        negate,
                        start,
                    } => {
                        let found = if *behind {
                            self.look_behind(haystack, *start, sp, anchor, &saves, &marks, budget)?
                        } else {
                            self.run(
                                haystack,
                                *start,
                                sp,
                                anchor,
                                saves.clone(),
                                marks.clone(),
                                budget,
                            )?
                        };
                        match (found, *negate) {
                            // A lookahead that held keeps its captures:
                            // `/(?=(a))/ =~ "a"` sets `$1`. A negative one
                            // that held matched nothing, so it sets nothing.
                            (Some((_, inner)), false) => {
                                saves = inner;
                                true
                            }
                            (None, true) => true,
                            _ => false,
                        }
                    }
                };

                if advanced {
                    pc += 1;
                    continue;
                }
                break true;
            };

            // The current path died. Take the most recent alternative.
            if failed {
                match stack.pop() {
                    Some(entry) => {
                        pc = entry.pc;
                        sp = entry.sp;
                        saves = entry.saves;
                        marks = entry.marks;
                    }
                    None => return Ok(None),
                }
            }
        }
    }

    /// Lookbehind, by trying every start position whose match could end at `sp`.
    ///
    /// Onigmo computes the body's possible widths and steps back exactly that
    /// far. This walks back one character at a time and asks the body to match
    /// and finish at `sp`, which is the same answer more slowly.
    ///
    /// ponytail: linear scan back to the start of the subject. Fine for the
    /// bounded lookbehinds Ruby allows; compute the body's min and max width if
    /// a pattern ever makes this the hot path.
    #[allow(clippy::too_many_arguments)]
    fn look_behind(
        &self,
        haystack: &str,
        start: usize,
        sp: usize,
        anchor: usize,
        saves: &[Option<usize>],
        marks: &[Option<Mark>],
        budget: &mut u64,
    ) -> Result<Option<SubMatch>, Error> {
        let mut from = sp;
        loop {
            if let Some((end, inner)) = self.run(
                haystack,
                start,
                from,
                anchor,
                saves.to_vec(),
                marks.to_vec(),
                budget,
            )? {
                // Only a body that finishes exactly where the lookbehind sits.
                if end == sp {
                    return Ok(Some((end, inner)));
                }
            }
            if from == 0 {
                return Ok(None);
            }
            from -= 1;
            while from > 0 && !haystack.is_char_boundary(from) {
                from -= 1;
            }
        }
    }
}

fn next_char(haystack: &str, sp: usize) -> Option<(char, usize)> {
    if sp >= haystack.len() {
        return None;
    }
    haystack[sp..].chars().next().map(|c| (c, c.len_utf8()))
}

fn prev_char(haystack: &str, sp: usize) -> Option<char> {
    haystack[..sp].chars().next_back()
}

fn chars_equal(a: char, b: char, icase: bool) -> bool {
    if a == b {
        return true;
    }
    // ponytail: simple case folding, one character to one character. Full
    // Unicode folding (ß to ss) needs a table; add it with the Encoding slice.
    icase && a.to_lowercase().eq(b.to_lowercase())
}

fn text_at(haystack: &str, sp: usize, text: &str, icase: bool) -> bool {
    let rest = &haystack[sp..];
    if !icase {
        return rest.starts_with(text);
    }
    let mut got = rest.chars();
    for want in text.chars() {
        match got.next() {
            Some(c) if chars_equal(c, want, true) => {}
            _ => return false,
        }
    }
    true
}

/// Ruby's `\b` is defined in terms of `\w`, which is ASCII.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn anchor_holds(anchor: Anchor, haystack: &str, sp: usize, start: usize) -> bool {
    match anchor {
        Anchor::TextStart => sp == 0,
        Anchor::TextEnd => sp == haystack.len(),
        Anchor::TextEndNewline => {
            sp == haystack.len() || (haystack[sp..].starts_with('\n') && sp + 1 == haystack.len())
        }
        Anchor::MatchStart => sp == start,
        // Always a line anchor, with no flag to say otherwise. The difference
        // from Rust's dialect, and half the reason this engine exists.
        //
        // The end of the subject is not the start of a line even when the
        // subject ends in a newline: "does not match ^ after trailing \n".
        Anchor::LineStart => {
            sp == 0 || (sp < haystack.len() && prev_char(haystack, sp) == Some('\n'))
        }
        Anchor::LineEnd => sp == haystack.len() || haystack[sp..].starts_with('\n'),
        Anchor::WordBoundary | Anchor::NotWordBoundary => {
            let before = prev_char(haystack, sp).is_some_and(is_word);
            let after = next_char(haystack, sp).is_some_and(|(c, _)| is_word(c));
            (before != after) == matches!(anchor, Anchor::WordBoundary)
        }
    }
}

fn class_matches(class: &Class, c: char) -> bool {
    let mut held = class
        .items
        .iter()
        .any(|item| item_matches(item, c, class.icase));
    // Case folding inside a class: `[a]` under `/i` also matches `A`.
    if !held && class.icase {
        held = c.to_lowercase().chain(c.to_uppercase()).any(|folded| {
            folded != c
                && class
                    .items
                    .iter()
                    .any(|item| item_matches(item, folded, false))
        });
    }
    // `[a-z&&[^b]]`: every intersected branch has to agree, and the `^`
    // negates the *whole* class including the intersection. Onigmo's
    // precedence, and the reason `[a-z&&[^d-i&&[^d-f]]]+` matches "abcdef"
    // rather than "abc" — `regexp_spec.rb`, "supports character class
    // composition".
    held = held && class.intersect.iter().all(|other| class_matches(other, c));
    if class.negated {
        held = !held;
    }
    held
}

fn item_matches(item: &ClassItem, c: char, icase: bool) -> bool {
    match item {
        ClassItem::Char(want) => chars_equal(c, *want, icase),
        ClassItem::Range(low, high) => {
            (*low..=*high).contains(&c)
                || (icase
                    && c.to_lowercase()
                        .chain(c.to_uppercase())
                        .any(|f| (*low..=*high).contains(&f)))
        }
        ClassItem::Perl(kind, negated) => perl_matches(*kind, c) != *negated,
        ClassItem::Posix(kind, negated) => posix_matches(*kind, c) != *negated,
        ClassItem::Nested(inner) => class_matches(inner, c),
    }
}

/// `\w`, `\d`, `\s`, `\h` — ASCII, always. Rust's `regex` crate makes these
/// Unicode-aware, which is one of the divergences `scripts/regexp-oracle.rb`
/// measured and this engine exists to avoid.
fn perl_matches(kind: Perl, c: char) -> bool {
    match kind {
        Perl::Word => c.is_ascii_alphanumeric() || c == '_',
        Perl::Digit => c.is_ascii_digit(),
        Perl::Space => matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'),
        Perl::Hex => c.is_ascii_hexdigit(),
    }
}

/// The POSIX brackets, which unlike the shorthands *are* Unicode-aware — the
/// same divergence in the other direction. Built on `char`'s own tables, so
/// they cost no dependency.
fn posix_matches(kind: Posix, c: char) -> bool {
    match kind {
        Posix::Alpha => c.is_alphabetic(),
        Posix::Digit => c.is_numeric(),
        Posix::Alnum => c.is_alphanumeric(),
        Posix::Upper => c.is_uppercase(),
        Posix::Lower => c.is_lowercase(),
        Posix::Space => c.is_whitespace(),
        // Onigmo's `blank` is horizontal space: tab plus every Zs. Small
        // enough to write out, and measured — `posix.txt`'s `blank` line is
        // this list. `char::is_whitespace` is the near miss it replaces: that
        // is Zs *plus* the line separators, so it says true for `\n`.
        Posix::Blank => {
            c == '\t'
                || matches!(
                    c,
                    '\u{20}' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                        ..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
                )
        }
        Posix::Print => !c.is_control(),
        Posix::Graph => !c.is_control() && !c.is_whitespace(),
        Posix::Cntrl => c.is_control(),
        Posix::XDigit => c.is_ascii_hexdigit(),
        Posix::Word => c.is_alphanumeric() || c == '_',
        Posix::Ascii => c.is_ascii(),
        // ponytail: ASCII punctuation plus "not anything else", which is the
        // shape of Unicode's P* without carrying the table for it.
        Posix::Punct => {
            c.is_ascii_punctuation()
                || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control())
        }
    }
}

/// The POSIX brackets, replayed against CRuby over every scalar value.
///
/// A unit test rather than one under `tests/` because the thing being checked
/// is [`posix_matches`] itself: going through the public API would compile a
/// pattern and run the machine 1.1 million times per bracket, which is the same
/// answer two orders of magnitude more slowly.
#[cfg(test)]
mod posix_oracle {
    use super::{Posix, posix_matches};

    /// Measured by `scripts/regexp-oracle.rb --generate`. CI re-checks it.
    const TABLE: &str = include_str!("../tests/posix.txt");

    fn kind(name: &str) -> Posix {
        match name {
            "alpha" => Posix::Alpha,
            "digit" => Posix::Digit,
            "alnum" => Posix::Alnum,
            "upper" => Posix::Upper,
            "lower" => Posix::Lower,
            "space" => Posix::Space,
            "blank" => Posix::Blank,
            "print" => Posix::Print,
            "graph" => Posix::Graph,
            "cntrl" => Posix::Cntrl,
            "xdigit" => Posix::XDigit,
            "word" => Posix::Word,
            "ascii" => Posix::Ascii,
            "punct" => Posix::Punct,
            other => panic!("posix.txt names a bracket the engine has no variant for: {other}"),
        }
    }

    /// `LO-HI;LO;...` in hex, as a lookup keyed by codepoint.
    fn ruby_set(fields: &str) -> Vec<bool> {
        let mut set = vec![false; 0x11_0000];
        for field in fields.split(';') {
            let (lo, hi) = match field.split_once('-') {
                Some((lo, hi)) => (lo, hi),
                None => (field, field),
            };
            let lo = u32::from_str_radix(lo, 16).expect("hex low bound");
            let hi = u32::from_str_radix(hi, 16).expect("hex high bound");
            for cp in lo..=hi {
                set[cp as usize] = true;
            }
        }
        set
    }

    /// The brackets that still answer differently from CRuby, with the exact
    /// number of codepoints each is wrong about, and why.
    ///
    /// All six are the same failure: `posix_matches` is built on `char`'s
    /// predicates, and Rust's unions of Unicode general categories are not
    /// Onigmo's. `Nd`, `M*`, `Pc`, `P*` and "assigned" are not reachable from
    /// `std`, so closing these needs a table from somewhere — which is a
    /// dependency decision rather than a session's, and is #180.
    ///
    /// Nothing is added here to make a run green. The list is printed on every
    /// run, and the test fails if an entry is stale as well as if a new
    /// disagreement appears: a bracket that has been fixed has to leave.
    const KNOWN_DIVERGENCES: &[(&str, usize)] = &[
        // In `posix.txt`'s own order, which is the order the test walks.
        //
        // `is_numeric` is Nd+Nl+No; Onigmo's digit is Nd alone.
        ("digit", 1154),
        // `is_alphanumeric` inherits that, so alnum is wrong wherever digit is.
        ("alnum", 915),
        // `!is_control` says an unassigned codepoint is printable. Ruby does
        // not, and most of Unicode's space is unassigned.
        ("print", 814732),
        ("graph", 814730),
        // Onigmo's word is Alphabetic + M* + Nd + Pc. Marks are the ones the
        // engine misses; No and Nl are the ones it adds.
        ("word", 2089),
        // Onigmo's punct is P*. The engine's "not anything else" catches every
        // symbol and every unassigned codepoint too.
        ("punct", 962009),
    ];

    #[test]
    fn every_bracket_agrees_with_cruby_on_every_codepoint() {
        let mut reports = Vec::new();
        let mut found: Vec<(String, usize)> = Vec::new();

        for line in TABLE.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let Some((name, fields)) = line.split_once('\t') else {
                continue;
            };
            let want = ruby_set(fields);
            let kind = kind(name);

            let mut wrong = 0usize;
            let mut first = Vec::new();
            for cp in 0..=0x10_FFFFu32 {
                // Not a scalar value: neither engine has an answer to compare.
                let Some(c) = char::from_u32(cp) else {
                    continue;
                };
                let got = posix_matches(kind, c);
                if got != want[cp as usize] {
                    wrong += 1;
                    if first.len() < 8 {
                        first.push(format!(
                            "U+{cp:04X} ruby {} engine {}",
                            want[cp as usize], got
                        ));
                    }
                }
            }
            if wrong > 0 {
                found.push((name.to_owned(), wrong));
                reports.push(format!(
                    "[[:{name}:]] disagrees on {wrong} codepoints\n    {}",
                    first.join("\n    ")
                ));
            }
        }

        for (name, count) in KNOWN_DIVERGENCES {
            println!("known divergence, still open (#180): [[:{name}:]] on {count} codepoints");
        }

        // One assertion covers all three ways this list can rot: a bracket that
        // newly disagrees, a bracket that was fixed and not deleted from the
        // list, and a count that drifted because Unicode moved under it.
        let known: Vec<(String, usize)> = KNOWN_DIVERGENCES
            .iter()
            .map(|(n, c)| ((*n).to_owned(), *c))
            .collect();
        assert_eq!(
            found,
            known,
            "the POSIX brackets that disagree with CRuby are not the ones #180 \
             wrote down. Fix the bracket, or fix the list — do not widen it to \
             make a run green.\n\n{}",
            reports.join("\n\n")
        );
    }
}
