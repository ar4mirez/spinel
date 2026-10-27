//! [`Ast`] to a program, and the backtracking machine that runs it.
//!
//! Backtracking rather than a Thompson NFA because Ruby's dialect has
//! backreferences and lookaround, neither of which a finite automaton can
//! express. The machine is iterative over an explicit stack for the main flow,
//! and recursive only for lookaround and atomic groups, whose nesting depth is
//! a property of the pattern rather than of the subject.

use crate::parse::{Anchor, Ast, Class, ClassItem, Perl, Posix};
use crate::{Charset, Encoding, Error};

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
            // The parser has replaced every call with the group it calls.
            Ast::Call(_) => unreachable!("a subexpression call left unresolved"),
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
///
/// Measured, and it is not the hot path yet — `bench/regex_captures.rs`, #184.
/// The whole capture machinery, clone included, is 11% of a three-group match
/// and 31% of the worst nine-group one; the same engine is 27x slower than
/// `ruby --yjit` on that pattern for reasons this clone is not. Narrowing it
/// buys back at most a third of a number that is off by more than an order of
/// magnitude elsewhere.
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
    pub fn find_at(&self, haystack: Subject<'_>, start: usize) -> Result<Option<Match>, Error> {
        let mut budget = MAX_STEPS;
        let mut at = start;
        loop {
            if let Some(m) = self.run_from(haystack, at, start, &mut budget)? {
                return Ok(Some(m));
            }
            if at >= haystack.len() {
                return Ok(None);
            }
            // Advance one character, never one byte: a match can only begin on
            // a character boundary.
            at += haystack.next_char(at).map_or(1, |(_, width)| width);
        }
    }

    /// One attempt at `start` and nowhere else, with `\G` anchored there:
    /// what `StringScanner#scan` asks.
    pub fn match_at(&self, haystack: Subject<'_>, start: usize) -> Result<Option<Match>, Error> {
        let mut budget = MAX_STEPS;
        self.run_from(haystack, start, start, &mut budget)
    }

    /// One attempt at `at`. `start` is where the whole search began, which is
    /// what `\G` anchors to — not `at`: measured, `'hello'.match(/\Go/)` is
    /// nil, and anchoring to the attempt matched the `o`.
    fn run_from(
        &self,
        haystack: Subject<'_>,
        at: usize,
        start: usize,
        budget: &mut u64,
    ) -> Result<Option<Match>, Error> {
        let saves = vec![None; (self.groups + 1) * 2];
        let marks = vec![None; self.marks];
        let Some((_, saves)) = self.run(haystack, 0, at, start, saves, marks, budget)? else {
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
        haystack: Subject<'_>,
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
                    Inst::Char { ch, icase } => match haystack.next_char(sp) {
                        Some((c, width))
                            if chars_equal(c, *ch, *icase, haystack.enc, Fold::Literal) =>
                        {
                            sp += width;
                            true
                        }
                        _ => false,
                    },
                    Inst::Any { nl } => match haystack.next_char(sp) {
                        Some((c, width)) if *nl || c != '\n' => {
                            sp += width;
                            true
                        }
                        _ => false,
                    },
                    Inst::Class(index) => {
                        let class = &self.classes[*index as usize];
                        match haystack.next_char(sp) {
                            Some((c, width)) if class_holds(class, c, haystack.enc) => {
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
                            // path. Measured and deferred with the clone above —
                            // see `bench/regex_captures.rs` and #184.
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
                                let text = haystack.bytes[s..e].to_owned();
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
        haystack: Subject<'_>,
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

/// The bytes a pattern runs over, and how to read characters out of them.
#[derive(Clone, Copy)]
pub struct Subject<'a> {
    pub bytes: &'a [u8],
    pub enc: Encoding,
}

impl Subject<'_> {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn is_char_boundary(&self, at: usize) -> bool {
        self.enc.is_char_boundary(self.bytes, at)
    }

    /// The character at `sp` and how many bytes it takes. A byte that does
    /// not start a valid character is never handed over by the VM, which
    /// refuses a broken subject before matching; it reads as one character
    /// that matches nothing a pattern can spell.
    fn next_char(&self, sp: usize) -> Option<(char, usize)> {
        self.enc.decode(self.bytes.get(sp..)?)
    }

    /// The character that ends at `sp`. In EUC-JP and Shift_JIS that means
    /// reading forward from somewhere known, because a trail byte can also be
    /// a lead, or an `a`.
    fn prev_char(&self, sp: usize) -> Option<char> {
        if sp == 0 {
            return None;
        }
        let from = self.enc.char_start_before(self.bytes, sp);
        self.next_char(from).map(|(c, _)| c)
    }
}

/// What a character past ASCII is to the classes and the case folds, which is
/// a property of the encoding it was read in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// ASCII, in any encoding, or anything in UTF-8: Unicode's classes.
    Unicode,
    /// A byte of BINARY: no class, no case — Onigmo's ASCII-8BIT answers
    /// false for every ctype above `0x7f`, even though the `char` it is
    /// carried as here would say otherwise.
    Byte,
    /// A byte of a single-byte encoding with a table: the table's classes.
    Single(Charset, u8),
    /// A character of EUC-JP or Shift_JIS past ASCII: word, graph and print,
    /// and nothing else; a case only by the encoding's own arithmetic.
    Japanese,
}

fn kind(c: char, enc: Encoding) -> Kind {
    if c.is_ascii() {
        return Kind::Unicode;
    }
    match enc {
        Encoding::Utf8 => Kind::Unicode,
        Encoding::Binary => Kind::Byte,
        Encoding::Single(set) => Kind::Single(set, u8::try_from(u32::from(c)).unwrap_or(0xff)),
        Encoding::EucJp | Encoding::ShiftJis => Kind::Japanese,
    }
}

/// A carried character as the byte it is in a single-byte encoding.
fn byte(c: char) -> Option<u8> {
    u8::try_from(u32::from(c)).ok()
}

/// Which of a single-byte encoding's two case maps a comparison takes:
/// Onigmo folds a literal by one and a character class by another, and they
/// differ (#372). Every other encoding has the one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fold {
    Literal,
    Class,
}

/// Whether the subject's `a` matches the pattern's `b`.
///
/// In a single-byte encoding with a table the cases are the table's. A
/// literal's are not symmetric — Windows-1251's `/\x80/i` matches `"\x90"`,
/// and `/\x90/i` does not match `"\x80"` — and reach into ASCII: ISO-8859-9's
/// `/i/i` matches `İ`. A class's are fewer: `/[\x80\x81]/i` does not match
/// `"\x90"`, nor `/[h-j]/i` `İ`. ASCII's own cases are Unicode's.
fn chars_equal(a: char, b: char, icase: bool, enc: Encoding, fold: Fold) -> bool {
    if a == b {
        return true;
    }
    if !icase {
        return false;
    }
    if let Encoding::Single(set) = enc
        && !(a.is_ascii() && b.is_ascii())
    {
        let table = set.table();
        return byte(a).zip(byte(b)).is_some_and(|(a, b)| match fold {
            Fold::Literal => table.literal_folds(b, a),
            Fold::Class => table.class_folds(b, a),
        });
    }
    if kind(a, enc) == Kind::Japanese || kind(b, enc) == Kind::Japanese {
        return enc.other_case(a) == Some(b);
    }
    if kind(a, enc) != Kind::Unicode || kind(b, enc) != Kind::Unicode {
        return false;
    }
    // ponytail: simple case folding, one character to one character. Full
    // Unicode folding (ß to ss) needs a table; add it with the Encoding slice.
    a.to_lowercase().eq(b.to_lowercase())
}

/// Whether `test` holds for some other case of the subject's `c`: some
/// character a pattern could spell that matches `c` under `/i`.
fn any_other_case(c: char, enc: Encoding, mut test: impl FnMut(char) -> bool) -> bool {
    if let Encoding::Single(set) = enc {
        let ascii = c.is_ascii_alphabetic().then(|| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        });
        let table = byte(c)
            .into_iter()
            .flat_map(|b| set.table().class_partners(b));
        return ascii.into_iter().chain(table.map(char::from)).any(test);
    }
    match kind(c, enc) {
        Kind::Byte | Kind::Single(..) => false,
        Kind::Japanese => enc.other_case(c).is_some_and(test),
        Kind::Unicode => c
            .to_lowercase()
            .chain(c.to_uppercase())
            .any(|folded| folded != c && test(folded)),
    }
}

fn text_at(haystack: Subject<'_>, sp: usize, text: &[u8], icase: bool) -> bool {
    let rest = &haystack.bytes[sp..];
    if !icase {
        return rest.starts_with(text);
    }
    let want = Subject {
        bytes: text,
        enc: haystack.enc,
    };
    let (mut at, mut got) = (0, sp);
    // Both sides are the subject's, so the fold is taken either way round:
    // Windows-1251's `(.)\1` matches `"\x90\x80"` as well as `"\x80\x90"`.
    while let Some((w, width)) = want.next_char(at) {
        match haystack.next_char(got) {
            Some((c, used))
                if chars_equal(c, w, true, haystack.enc, Fold::Literal)
                    || chars_equal(w, c, true, haystack.enc, Fold::Literal) =>
            {
                got += used;
            }
            _ => return false,
        }
        at += width;
    }
    true
}

/// Ruby's `\b` is defined in terms of a word character: ASCII's in UTF-8
/// and BINARY here, the encoding's own table in the other single-byte
/// encodings, and every multibyte character in EUC-JP and Shift_JIS, which
/// `\w` is not.
///
/// ponytail: CRuby's UTF-8 `\b` is Unicode's word, not ASCII's —
/// `/\b/ =~ "é"` is 0 there. Out of #339's scope; it moves with the rest of
/// the Onigmo dialect.
fn is_word(c: char, enc: Encoding) -> bool {
    match kind(c, enc) {
        Kind::Single(set, b) => set.table().word(b),
        Kind::Japanese => true,
        Kind::Unicode | Kind::Byte => c.is_ascii_alphanumeric() || c == '_',
    }
}

fn anchor_holds(anchor: Anchor, haystack: Subject<'_>, sp: usize, start: usize) -> bool {
    let bytes = haystack.bytes;
    match anchor {
        Anchor::TextStart => sp == 0,
        Anchor::TextEnd => sp == haystack.len(),
        Anchor::TextEndNewline => {
            sp == haystack.len() || (bytes[sp..].starts_with(b"\n") && sp + 1 == haystack.len())
        }
        Anchor::MatchStart => sp == start,
        // Always a line anchor, with no flag to say otherwise. The difference
        // from Rust's dialect, and half the reason this engine exists.
        //
        // The end of the subject is not the start of a line even when the
        // subject ends in a newline: "does not match ^ after trailing \n".
        Anchor::LineStart => sp == 0 || (sp < haystack.len() && bytes[sp - 1] == b'\n'),
        Anchor::LineEnd => sp == haystack.len() || bytes[sp..].starts_with(b"\n"),
        Anchor::WordBoundary | Anchor::NotWordBoundary => {
            let enc = haystack.enc;
            let before = haystack.prev_char(sp).is_some_and(|c| is_word(c, enc));
            let after = haystack.next_char(sp).is_some_and(|(c, _)| is_word(c, enc));
            (before != after) == matches!(anchor, Anchor::WordBoundary)
        }
    }
}

/// Whether a pattern's class matches `c`. Onigmo compiles a class of one
/// character, `[\x80]` or `[i-i]`, as that character, so under `/i` it folds
/// as a literal does; a nested class is merged into its parent first, so
/// `[[\x80]\x81]` folds as a class.
fn class_holds(class: &Class, c: char, enc: Encoding) -> bool {
    let only = match class.items.as_slice() {
        [ClassItem::Char(only)] => Some(*only),
        [ClassItem::Range(low, high)] if low == high => Some(*low),
        _ => None,
    };
    match only {
        Some(only) if class.icase && !class.negated && class.intersect.is_empty() => {
            chars_equal(c, only, true, enc, Fold::Literal)
        }
        _ => class_matches(class, c, enc),
    }
}

fn class_matches(class: &Class, c: char, enc: Encoding) -> bool {
    let mut held = class
        .items
        .iter()
        .any(|item| item_matches(item, c, class.icase, enc));
    // Case folding inside a class: `[a]` under `/i` also matches `A`.
    if !held && class.icase {
        held = any_other_case(c, enc, |folded| {
            class
                .items
                .iter()
                .any(|item| item_matches(item, folded, false, enc))
        });
    }
    // `[a-z&&[^b]]`: every intersected branch has to agree, and the `^`
    // negates the *whole* class including the intersection. Onigmo's
    // precedence, and the reason `[a-z&&[^d-i&&[^d-f]]]+` matches "abcdef"
    // rather than "abc" — `regexp_spec.rb`, "supports character class
    // composition".
    held = held
        && class
            .intersect
            .iter()
            .all(|other| class_matches(other, c, enc));
    if class.negated {
        held = !held;
    }
    held
}

fn item_matches(item: &ClassItem, c: char, icase: bool, enc: Encoding) -> bool {
    match item {
        ClassItem::Char(want) => chars_equal(c, *want, icase, enc, Fold::Class),
        ClassItem::Range(low, high) => {
            (*low..=*high).contains(&c)
                || (icase && any_other_case(c, enc, |f| (*low..=*high).contains(&f)))
        }
        ClassItem::Perl(perl, negated) => {
            let held = match kind(c, enc) {
                Kind::Single(set, b) => set.table().perl(*perl, b),
                _ => perl_matches(*perl, c),
            };
            held != *negated
        }
        ClassItem::Posix(posix, negated) => {
            let held = match kind(c, enc) {
                Kind::Unicode => posix_matches(*posix, c),
                Kind::Byte => false,
                Kind::Single(set, b) => set.table().posix(*posix, b),
                Kind::Japanese => matches!(posix, Posix::Word | Posix::Graph | Posix::Print),
            };
            held != *negated
        }
        ClassItem::Nested(inner) => class_matches(inner, c, enc),
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

// The six brackets whose meaning is a union of Unicode general categories, as
// sorted range tables generated from the measured `tests/posix.txt`.
//
// `char`'s predicates cannot express them: they are unions Rust chose for
// Rust's purposes, and Onigmo's are different ones — `Nd` alone rather than
// `Nd + Nl + No`, `P*` rather than "not anything else", "assigned minus `C*`"
// rather than "not a control". See `build.rs` for why this is generated from a
// measurement rather than taken from a Unicode-tables crate (#180).
include!(concat!(env!("OUT_DIR"), "/posix_tables.rs"));

/// Whether a sorted, disjoint range table contains `c`.
fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    ranges
        .binary_search_by(|&(low, high)| {
            if c < low {
                std::cmp::Ordering::Greater
            } else if c > high {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// The POSIX brackets, which unlike the shorthands *are* Unicode-aware — the
/// same divergence in the other direction.
///
/// Eight of the fourteen are exact on `char`'s own predicates and stay there:
/// cheaper than a binary search, and #178's audit measured them as agreeing on
/// every one of the 1,114,112 scalar values. The other six are the tables
/// above, because the unions Onigmo means are not ones `char` exposes.
fn posix_matches(kind: Posix, c: char) -> bool {
    match kind {
        Posix::Alpha => c.is_alphabetic(),
        Posix::Digit => in_ranges(DIGIT, c),
        Posix::Alnum => in_ranges(ALNUM, c),
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
        Posix::Print => in_ranges(PRINT, c),
        Posix::Graph => in_ranges(GRAPH, c),
        Posix::Cntrl => c.is_control(),
        Posix::XDigit => c.is_ascii_hexdigit(),
        Posix::Word => in_ranges(WORD, c),
        Posix::Ascii => c.is_ascii(),
        Posix::Punct => in_ranges(PUNCT, c),
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

    pub(super) fn kind(name: &str) -> Posix {
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
    /// Empty since #180. All six entries that used to be here were the same
    /// failure — `posix_matches` was built on `char`'s predicates, and Rust's
    /// unions of Unicode general categories are not Onigmo's — and all six were
    /// closed at once by generating range tables from this very file.
    ///
    /// Which makes the replay below partly a tautology, and it is worth saying
    /// so plainly: six of the fourteen brackets are now checked against the
    /// table they were generated from. The oracle that still bites is
    /// `scripts/regexp-oracle.rb --check`, which CI runs against a live CRuby —
    /// if Ruby's answer moves, `posix.txt` moves, the generated tables move with
    /// it, and this test goes on passing because the engine followed. What this
    /// test still catches on its own is the other eight, which are `char`
    /// predicates and could drift with a Rust upgrade.
    ///
    /// Nothing is added here to make a run green. The list is printed on every
    /// run, and the test fails if an entry is stale as well as if a new
    /// disagreement appears: a bracket that has been fixed has to leave.
    const KNOWN_DIVERGENCES: &[(&str, usize)] = &[];

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

/// The classes, word boundary and case folds of the characters past ASCII in
/// every single-byte encoding, EUC-JP and Shift_JIS, replayed against CRuby
/// (#339, #372).
///
/// A unit test for the reason [`posix_oracle`] is one: the predicates are what
/// is checked, over every character each encoding has.
#[cfg(test)]
mod encoding_oracle {
    use super::{Fold, chars_equal, class_holds, class_matches, is_word, item_matches};
    use crate::Encoding;
    use crate::parse::{Class, ClassItem, Perl};
    use std::collections::{BTreeSet, HashMap};

    /// Measured by `scripts/regexp-oracle.rb --generate`. CI re-checks it.
    const TABLE: &str = include_str!("../tests/encodings.txt");

    fn encoding(name: &str) -> Encoding {
        Encoding::single_byte(name).unwrap_or_else(|| match name {
            "EUC-JP" => Encoding::EucJp,
            "Windows-31J" | "Shift_JIS" => Encoding::ShiftJis,
            other => panic!("encodings.txt names an encoding the engine does not read: {other}"),
        })
    }

    fn single(enc: Encoding) -> bool {
        matches!(enc, Encoding::Binary | Encoding::Single(_))
    }

    /// Every character past ASCII, by its code, read the way a subject is.
    fn characters(enc: Encoding) -> Vec<(u32, char)> {
        let mut sequences: Vec<Vec<u8>> = Vec::new();
        let euc = 0xa1..=0xfe_u8;
        match enc {
            _ if single(enc) => sequences.extend((0x80..=0xff_u8).map(|b| vec![b])),
            Encoding::EucJp => {
                sequences.extend(euc.clone().map(|t| vec![0x8e, t]));
                for lead in euc.clone() {
                    sequences.extend(euc.clone().map(|t| vec![lead, t]));
                }
                for mid in euc.clone() {
                    sequences.extend(euc.clone().map(|t| vec![0x8f, mid, t]));
                }
            }
            _ => {
                sequences.extend((0xa1..=0xdf_u8).map(|b| vec![b]));
                for lead in (0x81..=0x9f_u8).chain(0xe0..=0xfc) {
                    let trails = (0x40..=0x7e_u8).chain(0x80..=0xfc);
                    sequences.extend(trails.map(|t| vec![lead, t]));
                }
            }
        }
        sequences
            .into_iter()
            .map(|bytes| {
                let (c, len) = enc.decode(&bytes).expect("a character");
                assert_eq!(len, bytes.len(), "{bytes:02X?} is one character");
                let code = bytes.iter().fold(0u32, |code, &b| code << 8 | u32::from(b));
                assert_eq!(enc.code(c), code, "{bytes:02X?} keeps its code");
                assert_eq!(enc.char_bytes(c), bytes, "{bytes:02X?} writes back");
                (code, c)
            })
            .collect()
    }

    fn holds(label: &str, c: char, enc: Encoding) -> bool {
        let perl = |kind| item_matches(&ClassItem::Perl(kind, false), c, false, enc);
        match label {
            "\\b" => is_word(c, enc),
            "\\w" => perl(Perl::Word),
            "\\d" => perl(Perl::Digit),
            "\\s" => perl(Perl::Space),
            "\\h" => perl(Perl::Hex),
            name => item_matches(
                &ClassItem::Posix(super::posix_oracle::kind(name), false),
                c,
                false,
                enc,
            ),
        }
    }

    fn hex(text: &str) -> u32 {
        u32::from_str_radix(text, 16).expect("a hex code")
    }

    /// `LO-HI;LO;...`, or `all` or `none` of `codes`.
    fn wanted(field: &str, codes: &[u32]) -> Vec<u32> {
        match field {
            "all" => codes.to_vec(),
            "none" | "" => Vec::new(),
            ranges => ranges
                .split(';')
                .flat_map(|range| {
                    let (lo, hi) = range.split_once('-').unwrap_or((range, range));
                    hex(lo)..=hex(hi)
                })
                .collect(),
        }
    }

    /// `LO-HI+D;...` as the pairs `(x, y)` that `/x/i` matches.
    fn folds(field: &str) -> BTreeSet<(u32, u32)> {
        let mut pairs = BTreeSet::new();
        for block in field.split(';').filter(|b| !b.is_empty()) {
            let at = block.rfind(['+', '-']).expect("a distance");
            let (range, distance) = block.split_at(at);
            let (lo, hi) = range.split_once('-').expect("a range");
            let distance = hex(&distance[1..]);
            for code in hex(lo)..=hex(hi) {
                let other = if block.as_bytes()[at] == b'+' {
                    code + distance
                } else {
                    code - distance
                };
                pairs.insert((code, other));
            }
        }
        pairs
    }

    /// The pairs the engine folds as a literal, `fold`, or as a class of
    /// more than one character, `class-fold`. A single-byte encoding's are
    /// every pair with a side past ASCII, the literal asked both as itself
    /// and as a class of one; the others' are each character's other case by
    /// the encoding's arithmetic, and only as a literal.
    fn engine_folds(
        enc: Encoding,
        label: &str,
        chars: &[(u32, char)],
        wrong: &mut Vec<String>,
    ) -> BTreeSet<(u32, u32)> {
        if !single(enc) {
            return chars
                .iter()
                .filter_map(|&(code, c)| enc.other_case(c).map(|other| (code, enc.code(other))))
                .collect();
        }
        let class = |items: Vec<ClassItem>| Class {
            negated: false,
            icase: true,
            items,
            intersect: Vec::new(),
        };
        let mut pairs = BTreeSet::new();
        for x in 0..=0xff_u8 {
            for y in 0..=0xff_u8 {
                if x == y || (x.is_ascii() && y.is_ascii()) {
                    continue;
                }
                let (cx, cy) = (char::from(x), char::from(y));
                let (held, other) = if label == "fold" {
                    (
                        chars_equal(cy, cx, true, enc, Fold::Literal),
                        class_holds(&class(vec![ClassItem::Range(cx, cx)]), cy, enc),
                    )
                } else {
                    (
                        class_matches(&class(vec![ClassItem::Char(cx)]), cy, enc),
                        class_matches(&class(vec![ClassItem::Range(cx, cx)]), cy, enc),
                    )
                };
                if held != other {
                    wrong.push(format!(
                        "{enc:?} {label} {x:02X}->{y:02X}: {held} and {other}"
                    ));
                }
                if held {
                    pairs.insert((u32::from(x), u32::from(y)));
                }
            }
        }
        pairs
    }

    #[test]
    fn every_class_and_fold_agrees_with_cruby() {
        let mut wrong = Vec::new();
        let mut lines: HashMap<&str, usize> = HashMap::new();
        for line in TABLE.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let (name, label, field) = (fields[0], fields[1], fields.get(2).copied().unwrap_or(""));
            let enc = encoding(name);
            if label == "same" {
                // Read exactly as the encoding it measures the same as.
                if enc != encoding(field) {
                    wrong.push(format!("{name} is not read as {field}"));
                }
                continue;
            }
            *lines.entry(name).or_default() += 1;
            let chars = characters(enc);
            if label == "fold" || label == "class-fold" {
                let want = folds(field);
                let got = engine_folds(enc, label, &chars, &mut wrong);
                for pair in want.symmetric_difference(&got) {
                    wrong.push(format!(
                        "{name} fold {:X}->{:X}: ruby {}, engine {}",
                        pair.0,
                        pair.1,
                        want.contains(pair),
                        got.contains(pair)
                    ));
                }
                continue;
            }
            let codes: Vec<u32> = chars.iter().map(|&(code, _)| code).collect();
            let want = wanted(field, &codes);
            let got: Vec<u32> = chars
                .iter()
                .filter(|&&(_, c)| holds(label, c, enc))
                .map(|&(code, _)| code)
                .collect();
            if got != want {
                wrong.push(format!(
                    "{name} {label}: ruby {} codes, engine {}",
                    want.len(),
                    got.len()
                ));
            }
        }
        for name in [
            "ASCII-8BIT",
            "ISO-8859-1",
            "ISO-8859-2",
            "KOI8-R",
            "EUC-JP",
            "Shift_JIS",
        ] {
            assert!(lines.contains_key(name), "encodings.txt measures {name}");
        }
        for (name, count) in &lines {
            // A single-byte encoding's class folds have a line of their own.
            let want = if single(encoding(name)) { 21 } else { 20 };
            assert_eq!(*count, want, "{name} has a line per class and its folds");
        }
        assert!(
            wrong.is_empty(),
            "disagreements with CRuby:\n{}",
            wrong.join("\n")
        );
    }
}
