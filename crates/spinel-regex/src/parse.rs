//! Ruby's regex dialect, parsed into [`Ast`].
//!
//! The dialect is Onigmo's, which is not Rust's and not PCRE's. Two differences
//! run through everything below and are the reason this file exists rather than
//! a translation layer over an existing crate:
//!
//! - `^` and `$` are *always* line anchors. There is no flag that turns them
//!   into string anchors; `\A` and `\z` are how you ask for those.
//! - `\w`, `\d`, `\s` and their negations are ASCII-only, while the POSIX
//!   bracket classes (`[[:alpha:]]`) are Unicode-aware. Rust's `regex` has this
//!   exactly backwards, which is measured in `scripts/regexp-oracle.rb`.
//!
//! Flags are *baked in* rather than tracked at match time: `(?i)` changes what
//! the parser writes into the nodes that follow it, so the matcher never has to
//! carry a flag register. That is also what makes `(?i:a)b` mean what it says.

use crate::property::Property;
use crate::{CharRange, Encoding, Error, Flags};
use std::collections::{HashMap, HashSet};

/// One parsed node.
#[derive(Debug, Clone, PartialEq)]
pub enum Ast {
    /// Matches nothing and consumes nothing. `()` and `a|` produce these.
    Empty,
    /// One character. `icase` is baked in from the flags in force here.
    Literal {
        ch: char,
        icase: bool,
    },
    /// A bracket class, a `\w`-style shorthand, or a POSIX bracket.
    Class(Class),
    /// `.` — `nl` is set under Ruby's `/m`, which is Rust's `s`.
    Any {
        nl: bool,
    },
    Concat(Vec<Ast>),
    Alt(Vec<Ast>),
    /// `index` is `None` for `(?:...)`, `Some(n)` for a capture.
    Group {
        index: Option<usize>,
        body: Box<Ast>,
    },
    /// `(?>...)` and the body of a possessive quantifier: matched once, and
    /// never reconsidered on backtracking.
    Atomic(Box<Ast>),
    Repeat {
        body: Box<Ast>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
    Anchor(Anchor),
    /// `\1`, and `\k<name>` once the name is resolved to its group.
    Backref {
        group: usize,
        icase: bool,
    },
    Look {
        behind: bool,
        negate: bool,
        body: Box<Ast>,
    },
    /// `\g<name>` or `\g<1>` as written; [`parse`] resolves every one to a
    /// [`Ast::CallGroup`] before the matcher sees the tree.
    Call(String),
    /// A call of group `n`, `0` being the whole pattern.
    CallGroup(usize),
    /// `\k<name+n>` and `\k<name-n>`: group `group` as captured `level`
    /// calls away, `-1` out to the caller.
    LevelBackref {
        group: usize,
        icase: bool,
        level: isize,
    },
    /// `(?~absent)`.
    Absent(Box<Ast>),
    /// `\X`, one extended grapheme cluster.
    Grapheme,
    /// `(?(1)yes|no)`: `yes` if group `group` has matched, `no` otherwise.
    Cond {
        group: usize,
        yes: Box<Ast>,
        no: Box<Ast>,
    },
    /// `\K`: the match is reported as starting here.
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// `^` — start of the subject or just after a newline.
    LineStart,
    /// `$` — end of the subject, or before a newline at the end of a line.
    LineEnd,
    /// `\A`
    TextStart,
    /// `\z`
    TextEnd,
    /// `\Z` — end of the subject, or before a final newline.
    TextEndNewline,
    /// `\b`
    WordBoundary,
    /// `\B`
    NotWordBoundary,
    /// `\G` — where this match attempt started.
    MatchStart,
}

/// One bracket class, or one shorthand written as if it were bracketed.
#[derive(Debug, Clone, PartialEq)]
pub struct Class {
    pub negated: bool,
    pub icase: bool,
    pub items: Vec<ClassItem>,
    /// `[a-z&&[^bc]]` — every branch of the intersection must also match.
    pub intersect: Vec<Class>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassItem {
    Char(char),
    Range(char, char),
    /// `\w`, `\d`, `\s`, `\h` and their uppercase negations. ASCII-only, which
    /// is the whole point of spelling them out separately from POSIX.
    Perl(Perl, bool),
    /// `[[:alpha:]]` and friends. Unicode-aware.
    Posix(Posix, bool),
    /// `\p{L}` and its negations, in a UTF-8 pattern: a measured Unicode
    /// property, which `(?a)` leaves alone.
    Property(Property, bool),
    /// A nested `[...]`, which Onigmo allows inside a class.
    Nested(Box<Class>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perl {
    Word,
    Digit,
    Space,
    Hex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posix {
    Alpha,
    Digit,
    Alnum,
    Upper,
    Lower,
    Space,
    Blank,
    Punct,
    Print,
    Graph,
    Cntrl,
    XDigit,
    Word,
    Ascii,
}

/// What a successful parse hands back.
pub struct Parsed {
    pub ast: Ast,
    /// Number of capturing groups, so the matcher can size its save slots.
    pub groups: usize,
    /// `(?<name>)` in source order, mapped to the group each names. A name may
    /// be used more than once in Ruby, so this is a name to *last* group map,
    /// matching what `MatchData#[]` answers.
    pub names: Vec<(String, usize)>,
}

pub fn parse(source: &str, flags: Flags, encoding: Encoding) -> Result<Parsed, Error> {
    let mut parser = Parser {
        chars: source.chars().collect(),
        pos: 0,
        flags,
        encoding,
        groups: 0,
        names: Vec::new(),
        name_index: HashMap::new(),
        numeric_backref: false,
        conditioned: 0,
    };
    let ast = parser.alternation()?;
    if parser.pos < parser.chars.len() {
        // The only way to stop early is an unbalanced `)`.
        return Err(Error::Syntax("unmatched close parenthesis".into()));
    }
    // One named group anywhere in the pattern turns every numeric
    // backreference into an error — including one that refers to an unnamed
    // group declared before it, and one written before the named group is
    // reached. That is Ruby, measured: `/(a)(?<b>b)\1/`, `/(a)\1(?<b>b)/` and
    // `/\1(?<a>a)/` are all refused, while `/(a)\1/` is fine.
    //
    // The reason underneath is that a pattern with named groups does not number
    // its unnamed ones at all — `/(a)(?<b>b)/.match("ab").to_a` is `["ab", "b"]`
    // in Ruby — so a number has nothing to refer to. `drop_unnamed_groups`
    // below is #217 making that true here as well, but the check still cannot
    // fall out of it: after renumbering, `\1` in `/\1(?<a>a)/` *would* name a
    // group that exists, and Ruby refuses it anyway.
    if parser.numeric_backref && !parser.name_index.is_empty() {
        return Err(Error::Syntax(
            "numbered backref/call is not allowed. (use name)".into(),
        ));
    }
    // `(?(1)...)` may test a group written after it, but not one that is never
    // written at all.
    if parser.conditioned > parser.groups {
        return Err(Error::Syntax("invalid backref number/name".into()));
    }
    let mut ast = ast;
    resolve_calls(&mut ast, &parser.name_index)?;
    let (groups, names) = drop_unnamed_groups(&mut ast, parser.groups, parser.names);
    Ok(Parsed { ast, groups, names })
}

/// Ruby's rule: one named group anywhere makes every *unnamed* `(...)` a
/// plain group rather than a capture.
///
/// `/(a)(?<b>b)/.match("ab").to_a` is `["ab", "b"]`, not `["ab", "a", "b"]`,
/// and `MatchData#size` counts only the named ones. Measured. The remaining
/// groups are then numbered 1..k in source order, so `$~[1]` is the *named*
/// group — which is also why a numeric backreference is refused above: with
/// the unnamed groups gone, a number has nothing left to refer to.
///
/// A pass rather than a rule inside the parser because it is a property of the
/// whole pattern: `/(a)(?<b>b)/` un-numbers a group written before the parser
/// had any way to know a name was coming.
fn drop_unnamed_groups(
    ast: &mut Ast,
    groups: usize,
    names: Vec<(String, usize)>,
) -> (usize, Vec<(String, usize)>) {
    if names.is_empty() {
        return (groups, names);
    }
    // Old index to new, `None` for a group that stops being one. Named groups
    // keep their relative order, which is source order.
    let mut named: Vec<usize> = names.iter().map(|(_, index)| *index).collect();
    named.sort_unstable();
    let renumber =
        |old: usize| -> Option<usize> { named.binary_search(&old).ok().map(|at| at + 1) };
    renumber_groups(ast, &renumber);
    let names = names
        .into_iter()
        .map(|(name, index)| {
            let index = renumber(index).expect("a named group is in the named list");
            (name, index)
        })
        .collect();
    (named.len(), names)
}

/// Resolve every subexpression call to the group it calls, which the matcher
/// runs as a subroutine, as Onigmo does: so a call may recurse, and a level
/// backreference can tell one call's capture from another's.
///
/// A call that can reach itself again before matching a character never
/// ends, and Onigmo refuses the pattern: `/(?<a>a|\g<a>)/`. Measured.
fn resolve_calls(ast: &mut Ast, names: &HashMap<String, usize>) -> Result<(), Error> {
    if !contains_call(ast) {
        return Ok(());
    }
    let mut groups: HashMap<usize, Ast> = HashMap::new();
    collect_groups(ast, &mut groups);
    resolve(ast, names, &groups)?;
    let mut groups: HashMap<usize, Ast> = HashMap::new();
    collect_groups(ast, &mut groups);
    groups.insert(0, ast.clone());
    if never_ends(&groups) {
        return Err(Error::Syntax("never ending recursion".into()));
    }
    Ok(())
}

fn contains_call(ast: &Ast) -> bool {
    match ast {
        Ast::Call(_) => true,
        Ast::Concat(parts) | Ast::Alt(parts) => parts.iter().any(contains_call),
        Ast::Cond { yes, no, .. } => contains_call(yes) || contains_call(no),
        Ast::Group { body, .. }
        | Ast::Atomic(body)
        | Ast::Repeat { body, .. }
        | Ast::Look { body, .. }
        | Ast::Absent(body) => contains_call(body),
        _ => false,
    }
}

fn collect_groups(ast: &Ast, groups: &mut HashMap<usize, Ast>) {
    match ast {
        Ast::Group { index, body } => {
            if let Some(index) = index {
                groups.insert(*index, (**body).clone());
            }
            collect_groups(body, groups);
        }
        Ast::Concat(parts) | Ast::Alt(parts) => {
            parts.iter().for_each(|part| collect_groups(part, groups))
        }
        Ast::Cond { yes, no, .. } => {
            collect_groups(yes, groups);
            collect_groups(no, groups);
        }
        Ast::Atomic(body)
        | Ast::Repeat { body, .. }
        | Ast::Look { body, .. }
        | Ast::Absent(body) => collect_groups(body, groups),
        _ => {}
    }
}

/// Every `Ast::Call` to the `Ast::CallGroup` it names.
fn resolve(
    ast: &mut Ast,
    names: &HashMap<String, usize>,
    groups: &HashMap<usize, Ast>,
) -> Result<(), Error> {
    match ast {
        Ast::Call(name) => {
            let index = match name.parse::<usize>() {
                Ok(n) => n,
                Err(_) => *names
                    .get(name.as_str())
                    .ok_or_else(|| Error::Syntax(format!("undefined name <{name}> reference")))?,
            };
            if index != 0 && !groups.contains_key(&index) {
                return Err(Error::Syntax(format!("undefined group <{name}> reference")));
            }
            *ast = Ast::CallGroup(index);
            Ok(())
        }
        Ast::Concat(parts) | Ast::Alt(parts) => {
            for part in parts {
                resolve(part, names, groups)?;
            }
            Ok(())
        }
        Ast::Cond { yes, no, .. } => {
            resolve(yes, names, groups)?;
            resolve(no, names, groups)
        }
        Ast::Group { body, .. }
        | Ast::Atomic(body)
        | Ast::Repeat { body, .. }
        | Ast::Look { body, .. }
        | Ast::Absent(body) => resolve(body, names, groups),
        _ => Ok(()),
    }
}

/// Whether some group can call itself again before matching a character,
/// through the groups it calls first. Which groups may match empty is settled
/// first, by [`empty_groups`]; then each body is walked once for the calls it
/// makes first, and the answer is a cycle search over those calls.
fn never_ends(groups: &HashMap<usize, Ast>) -> bool {
    let empty = empty_groups(groups);
    let firsts: HashMap<usize, Vec<usize>> = groups
        .iter()
        .map(|(&group, body)| {
            let mut calls = Vec::new();
            first_calls(body, &empty, &mut calls);
            (group, calls)
        })
        .collect();
    let mut done = HashMap::new();
    groups
        .keys()
        .any(|&group| on_cycle(group, &firsts, &mut done))
}

/// Depth-first from `group`: whether it reaches a group still being followed.
/// `done` holds each group already settled, `false` while it is open.
fn on_cycle(
    group: usize,
    firsts: &HashMap<usize, Vec<usize>>,
    done: &mut HashMap<usize, bool>,
) -> bool {
    if let Some(&settled) = done.get(&group) {
        return !settled;
    }
    done.insert(group, false);
    let cycle = firsts
        .get(&group)
        .is_some_and(|calls| calls.iter().any(|&next| on_cycle(next, firsts, done)));
    done.insert(group, true);
    cycle
}

/// The groups whose bodies can match without consuming a character, as a
/// least fixpoint: a group counts as consuming until its body is shown not to.
/// Each body is walked once, and again only when a group it calls turns out
/// to be empty.
fn empty_groups(groups: &HashMap<usize, Ast>) -> HashSet<usize> {
    let mut callers: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&group, body) in groups {
        let mut calls = Vec::new();
        all_calls(body, &mut calls);
        calls.sort_unstable();
        calls.dedup();
        for callee in calls {
            callers.entry(callee).or_default().push(group);
        }
    }
    let mut empty = HashSet::new();
    let mut pending: Vec<usize> = groups.keys().copied().collect();
    while let Some(group) = pending.pop() {
        if empty.contains(&group) || !may_be_empty(&groups[&group], &empty) {
            continue;
        }
        empty.insert(group);
        if let Some(callers) = callers.get(&group) {
            pending.extend(callers.iter().filter(|caller| !empty.contains(caller)));
        }
    }
    empty
}

/// Every group `ast` calls, wherever the call sits.
fn all_calls(ast: &Ast, calls: &mut Vec<usize>) {
    match ast {
        Ast::CallGroup(group) => calls.push(*group),
        Ast::Concat(parts) | Ast::Alt(parts) => {
            parts.iter().for_each(|part| all_calls(part, calls))
        }
        Ast::Cond { yes, no, .. } => {
            all_calls(yes, calls);
            all_calls(no, calls);
        }
        Ast::Group { body, .. }
        | Ast::Atomic(body)
        | Ast::Repeat { body, .. }
        | Ast::Look { body, .. }
        | Ast::Absent(body) => all_calls(body, calls),
        _ => {}
    }
}

/// The groups `ast` can call before it has matched a character.
fn first_calls(ast: &Ast, empty: &HashSet<usize>, calls: &mut Vec<usize>) {
    match ast {
        Ast::CallGroup(group) => calls.push(*group),
        Ast::Concat(parts) => {
            for part in parts {
                first_calls(part, empty, calls);
                if !may_be_empty(part, empty) {
                    break;
                }
            }
        }
        Ast::Alt(parts) => parts
            .iter()
            .for_each(|part| first_calls(part, empty, calls)),
        Ast::Cond { yes, no, .. } => {
            first_calls(yes, empty, calls);
            first_calls(no, empty, calls);
        }
        Ast::Group { body, .. }
        | Ast::Atomic(body)
        | Ast::Repeat { body, .. }
        | Ast::Look { body, .. }
        | Ast::Absent(body) => first_calls(body, empty, calls),
        _ => {}
    }
}

/// Whether `ast` can match without consuming a character, given the groups
/// known to.
fn may_be_empty(ast: &Ast, empty: &HashSet<usize>) -> bool {
    match ast {
        Ast::Literal { .. } | Ast::Class(_) | Ast::Any { .. } | Ast::Grapheme => false,
        Ast::Concat(parts) => parts.iter().all(|part| may_be_empty(part, empty)),
        Ast::Alt(parts) => parts.iter().any(|part| may_be_empty(part, empty)),
        Ast::Group { body, .. } | Ast::Atomic(body) => may_be_empty(body, empty),
        Ast::Repeat { body, min, .. } => *min == 0 || may_be_empty(body, empty),
        Ast::CallGroup(group) => empty.contains(group),
        _ => true,
    }
}

/// Rewrite every group index and backreference through `renumber`.
fn renumber_groups(ast: &mut Ast, renumber: &impl Fn(usize) -> Option<usize>) {
    match ast {
        Ast::Group { index, body } => {
            if let Some(old) = *index {
                *index = renumber(old);
            }
            renumber_groups(body, renumber);
        }
        // A numeric backreference or call is already an error in a pattern
        // with names, so this only ever resolves a name — which points at a
        // named group and therefore always renumbers. `\g<0>` is the whole
        // pattern under any numbering.
        Ast::Backref { group, .. } | Ast::LevelBackref { group, .. } => {
            if let Some(new) = renumber(*group) {
                *group = new;
            }
        }
        Ast::CallGroup(group) => {
            if *group != 0
                && let Some(new) = renumber(*group)
            {
                *group = new;
            }
        }
        Ast::Cond { group, yes, no } => {
            // The same holds for a condition: only a name survives to here.
            if let Some(new) = renumber(*group) {
                *group = new;
            }
            renumber_groups(yes, renumber);
            renumber_groups(no, renumber);
        }
        Ast::Concat(parts) | Ast::Alt(parts) => {
            for part in parts {
                renumber_groups(part, renumber);
            }
        }
        Ast::Atomic(body)
        | Ast::Repeat { body, .. }
        | Ast::Look { body, .. }
        | Ast::Absent(body) => {
            renumber_groups(body, renumber);
        }
        Ast::Empty
        | Ast::Literal { .. }
        | Ast::Class(_)
        | Ast::Any { .. }
        | Ast::Anchor(_)
        | Ast::Call(_)
        | Ast::Keep
        | Ast::Grapheme => {}
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    flags: Flags,
    /// What the source is written in, which decides what `\xff` is.
    encoding: Encoding,
    groups: usize,
    names: Vec<(String, usize)>,
    name_index: HashMap<String, usize>,
    /// Whether a numeric backreference — `\1` or `\k<1>` — was written
    /// anywhere in the pattern.
    ///
    /// Checked once at the end rather than where it is read, because the rule
    /// it feeds is a property of the whole pattern: one named group makes every
    /// numeric backreference an error, including one written *before* the named
    /// group appears. Measured — `/\1(?<a>a)/` is refused too (#205).
    numeric_backref: bool,
    /// The highest group a `(?(n)...)` condition names by number, checked
    /// against the group count once the whole pattern is read.
    conditioned: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Under `/x`, whitespace and `# ...` comments are not pattern. This runs
    /// between tokens, never inside a bracket class, which is where `/x` stops
    /// applying.
    fn skip_extended(&mut self) {
        if !self.flags.extended {
            return;
        }
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.pos += 1;
                }
                Some('#') => {
                    while let Some(c) = self.peek() {
                        self.pos += 1;
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => return,
            }
        }
    }

    fn alternation(&mut self) -> Result<Ast, Error> {
        let mut branches = vec![self.concat()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.concat()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().expect("just checked there is one")
        } else {
            Ast::Alt(branches)
        })
    }

    fn concat(&mut self) -> Result<Ast, Error> {
        let mut items: Vec<Ast> = Vec::new();
        loop {
            self.skip_extended();
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let atom = self.atom()?;
            // A `(?i)` mid-pattern parses as `Empty` after mutating flags; it
            // must not then pick up a quantifier of its own.
            let quantified = if matches!(atom, Ast::Empty) {
                atom
            } else {
                self.quantifier(atom)?
            };
            items.push(quantified);
        }
        Ok(match items.len() {
            0 => Ast::Empty,
            1 => items.pop().expect("just checked there is one"),
            _ => Ast::Concat(items),
        })
    }

    /// Zero or more quantifiers stacked on one atom.
    ///
    /// `a*?` is lazy and `a*+` is possessive, but `a{2}?` is neither: after the
    /// *exact* `{n}` form Ruby reads the `?` as a second quantifier, so `a{2}?`
    /// is `(a{2})?` and `a{2}+` is one-or-more of `a{2}`. Measured, not
    /// guessed — the comma is what makes the difference, and only `{n}` has
    /// none. `repetition_spec.rb` checks the `?` case.
    fn quantifier(&mut self, mut atom: Ast) -> Result<Ast, Error> {
        loop {
            self.skip_extended();
            let (min, max, exact) = match self.peek() {
                Some('*') => {
                    self.pos += 1;
                    (0, None, false)
                }
                Some('+') => {
                    self.pos += 1;
                    (1, None, false)
                }
                Some('?') => {
                    self.pos += 1;
                    (0, Some(1), false)
                }
                Some('{') => match self.bounded_repeat()? {
                    Some(bounds) => bounds,
                    // `a{` with no closing brace is a literal `{` in Ruby.
                    None => return Ok(atom),
                },
                _ => return Ok(atom),
            };

            // `??` and `*?` are lazy; `?+` and `*+` are possessive. After an
            // exact `{n}` neither applies: the next character starts a new
            // quantifier, which the enclosing loop picks up.
            let mut greedy = true;
            let mut possessive = false;
            if !exact {
                match self.peek() {
                    Some('?') => {
                        self.pos += 1;
                        greedy = false;
                    }
                    Some('+') => {
                        self.pos += 1;
                        possessive = true;
                    }
                    _ => {}
                }
            }

            atom = Ast::Repeat {
                body: Box::new(atom),
                min,
                max,
                greedy,
            };
            if possessive {
                atom = Ast::Atomic(Box::new(atom));
            }
        }
    }

    /// `{n}`, `{n,}`, `{,m}`, `{n,m}`. The third field says whether the form
    /// was the exact `{n}`, which alone refuses a lazy or possessive suffix.
    ///
    /// Answers `None` when what follows `{` is not a repeat at all, leaving the
    /// position untouched so `{` falls through to being a literal.
    fn bounded_repeat(&mut self) -> Result<Option<(u32, Option<u32>, bool)>, Error> {
        let start = self.pos;
        self.pos += 1; // the `{`
        let mut low = String::new();
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            low.push(self.next().expect("just peeked"));
        }
        let (min, max, exact) = if self.eat(',') {
            let mut high = String::new();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                high.push(self.next().expect("just peeked"));
            }
            // `{,}` bounds nothing, so it is not a quantifier at all — but
            // `{,2}` is one. Measured, because the two look alike.
            if low.is_empty() && high.is_empty() {
                self.pos = start;
                return Ok(None);
            }
            let min = low.parse::<u32>().unwrap_or(0);
            let max = if high.is_empty() {
                None
            } else {
                Some(
                    high.parse::<u32>()
                        .map_err(|_| Error::Syntax("repeat count too large".into()))?,
                )
            };
            (min, max, false)
        } else if low.is_empty() {
            // `{}` or `{a}`: not a quantifier.
            self.pos = start;
            return Ok(None);
        } else {
            let n = low
                .parse::<u32>()
                .map_err(|_| Error::Syntax("repeat count too large".into()))?;
            (n, Some(n), true)
        };
        if !self.eat('}') {
            self.pos = start;
            return Ok(None);
        }
        if let Some(max) = max {
            if max < min {
                return Err(Error::Syntax("min repeat greater than max repeat".into()));
            }
        }
        Ok(Some((min, max, exact)))
    }

    fn atom(&mut self) -> Result<Ast, Error> {
        let Some(c) = self.next() else {
            return Ok(Ast::Empty);
        };
        match c {
            '.' => Ok(Ast::Any {
                nl: self.flags.dotall,
            }),
            '^' => Ok(Ast::Anchor(Anchor::LineStart)),
            '$' => Ok(Ast::Anchor(Anchor::LineEnd)),
            '[' => Ok(Ast::Class(self.bracket_class()?)),
            '(' => self.group(),
            ')' => Err(Error::Syntax("unmatched close parenthesis".into())),
            '*' | '+' | '?' => Err(Error::Syntax(
                "target of repeat operator is not specified".into(),
            )),
            '\\' => self.escape(),
            _ => Ok(self.literal(c)),
        }
    }

    fn literal(&self, ch: char) -> Ast {
        Ast::Literal {
            ch,
            icase: self.flags.icase,
        }
    }

    /// Everything after a `(`.
    fn group(&mut self) -> Result<Ast, Error> {
        if !self.eat('?') {
            self.groups += 1;
            let index = self.groups;
            let body = self.grouped_alternation()?;
            return Ok(Ast::Group {
                index: Some(index),
                body: Box::new(body),
            });
        }

        match self.peek() {
            Some(':') => {
                self.pos += 1;
                let body = self.grouped_alternation()?;
                Ok(Ast::Group {
                    index: None,
                    body: Box::new(body),
                })
            }
            Some('=') => {
                self.pos += 1;
                self.lookaround(false, false)
            }
            Some('!') => {
                self.pos += 1;
                self.lookaround(false, true)
            }
            Some('>') => {
                self.pos += 1;
                let body = self.grouped_alternation()?;
                Ok(Ast::Atomic(Box::new(body)))
            }
            Some('#') => {
                // `(?#...)` is a comment.
                while let Some(c) = self.next() {
                    if c == ')' {
                        return Ok(Ast::Empty);
                    }
                }
                Err(Error::Syntax("end pattern in group".into()))
            }
            Some('<') => match self.peek_at(1) {
                Some('=') => {
                    self.pos += 2;
                    self.lookaround(true, false)
                }
                Some('!') => {
                    self.pos += 2;
                    self.lookaround(true, true)
                }
                _ => {
                    self.pos += 1;
                    self.named_group('>')
                }
            },
            Some('\'') => {
                self.pos += 1;
                self.named_group('\'')
            }
            // `(?~...)`, `(?(1)...)`, `(?{...})` and the other Onigmo corners.
            // Refused rather than approximated: see the crate docs.
            Some('~') => {
                self.pos += 1;
                let body = self.grouped_alternation()?;
                Ok(Ast::Absent(Box::new(body)))
            }
            Some('(') => {
                self.pos += 1;
                self.conditional()
            }
            Some('{') => Err(Error::Unsupported("an embedded code block (?{...})")),
            _ => self.inline_flags(),
        }
    }

    /// `(?(cond)yes|no)`, from just after the `(?(`. The condition names a
    /// group: `1`, `<1>`, `'1'`, `<-1>`, `<name>` or `'name'`. A bare name is
    /// refused, as is a lookaround, and so is a third branch.
    fn conditional(&mut self) -> Result<Ast, Error> {
        let invalid = || Error::Syntax("invalid conditional pattern".into());
        let group = match self.peek() {
            Some(open @ ('<' | '\'')) => {
                self.pos += 1;
                let close = if open == '<' { '>' } else { '\'' };
                let name = self.delimited_name(close).ok_or_else(invalid)?;
                let group = self.reference(&name)?;
                self.conditioned = self.conditioned.max(group);
                group
            }
            Some(c) if c.is_ascii_digit() => {
                let mut digits = String::new();
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    digits.push(self.next().expect("just peeked"));
                }
                let group = self.reference(&digits)?;
                self.conditioned = self.conditioned.max(group);
                group
            }
            _ => return Err(invalid()),
        };
        if !self.eat(')') {
            return Err(invalid());
        }
        let (yes, no) = match self.grouped_alternation()? {
            Ast::Alt(mut branches) if branches.len() == 2 => {
                let no = branches.pop().expect("two branches");
                let yes = branches.pop().expect("two branches");
                (yes, no)
            }
            Ast::Alt(_) => return Err(invalid()),
            yes => (yes, Ast::Empty),
        };
        Ok(Ast::Cond {
            group,
            yes: Box::new(yes),
            no: Box::new(no),
        })
    }

    /// The name between `<` and `>`, or between quotes, consumed with its
    /// closing delimiter. `None` if the pattern ends first.
    fn delimited_name(&mut self, close: char) -> Option<String> {
        let mut name = String::new();
        loop {
            match self.next()? {
                c if c == close => return Some(name),
                c => name.push(c),
            }
        }
    }

    /// The group a `\k<...>` or a `(?(<...>)...)` names: a number, a number
    /// relative to the groups opened so far (`-1` is the last one), or a name,
    /// any of them optionally with a recursion level (`name+0`, `1+0`). Messages are Onigmo's.
    fn reference(&mut self, name: &str) -> Result<usize, Error> {
        match self.leveled_reference(name)? {
            (group, None | Some(0)) => Ok(group),
            (_, Some(_)) => Err(Error::Unsupported("a level in a conditional's group")),
        }
    }

    /// [`Parser::reference`], with the level a `name+n` or `name-n` gives.
    fn leveled_reference(&mut self, name: &str) -> Result<(usize, Option<isize>), Error> {
        let invalid_name = || Error::Syntax(format!("invalid group name <{name}>"));
        let undefined = |base: &str| Error::Syntax(format!("undefined name <{base}> reference"));
        // Before the number, which Rust would read `+1` as.
        if name.starts_with('+') {
            return Err(undefined(name));
        }
        // `name+1` or `name-1`: a capture at a recursion level. A `+` or `-`
        // not followed by a level is no name at all, even when a group is
        // called exactly that — `(?<a+>a)` is legal and `\k<a+>` is not.
        if let Some(at) = name.rfind(['+', '-']).filter(|&at| at > 0) {
            let (base, level) = (&name[..at], &name[at + 1..]);
            let Ok(level) = level.parse::<isize>() else {
                return Err(invalid_name());
            };
            let level = if name.as_bytes()[at] == b'-' {
                -level
            } else {
                level
            };
            let group = self.reference(base)?;
            return Ok((group, Some(level)));
        }
        if let Ok(n) = name.parse::<usize>() {
            // `\k<0>` names the whole match rather than a group, and Ruby
            // refuses the pattern rather than matching nothing with it.
            if n == 0 {
                return Err(invalid_name());
            }
            self.numeric_backref = true;
            return Ok((n, None));
        }
        if let Some(back) = name.strip_prefix('-') {
            let n = back.parse::<usize>().map_err(|_| invalid_name())?;
            if n == 0 {
                return Err(invalid_name());
            }
            if n > self.groups {
                return Err(Error::Syntax("invalid backref number/name".into()));
            }
            self.numeric_backref = true;
            return Ok((self.groups + 1 - n, None));
        }
        self.name_index
            .get(name)
            .copied()
            .map(|group| (group, None))
            .ok_or_else(|| undefined(name))
    }

    fn lookaround(&mut self, behind: bool, negate: bool) -> Result<Ast, Error> {
        let body = self.grouped_alternation()?;
        Ok(Ast::Look {
            behind,
            negate,
            body: Box::new(body),
        })
    }

    fn named_group(&mut self, close: char) -> Result<Ast, Error> {
        let mut name = String::new();
        loop {
            match self.next() {
                Some(c) if c == close => break,
                Some(c) => name.push(c),
                None => {
                    return Err(Error::Syntax(
                        "end pattern with unmatched parenthesis".into(),
                    ));
                }
            }
        }
        if name.is_empty() {
            return Err(Error::Syntax("group name is empty".into()));
        }
        // `group names cannot start with digits or minus`, per grouping_spec.
        let first = name.chars().next().expect("just checked non-empty");
        if first.is_ascii_digit() || first == '-' {
            return Err(Error::Syntax("invalid group name".into()));
        }
        self.groups += 1;
        let index = self.groups;
        self.name_index.insert(name.clone(), index);
        self.names.push((name, index));
        let body = self.grouped_alternation()?;
        Ok(Ast::Group {
            index: Some(index),
            body: Box::new(body),
        })
    }

    /// `(?imx-imx)` sets flags for the rest of the enclosing group;
    /// `(?imx-imx:...)` scopes them to the parenthesised body.
    fn inline_flags(&mut self) -> Result<Ast, Error> {
        let saved = self.flags;
        let mut flags = self.flags;
        let mut negating = false;
        let mut seen = false;
        loop {
            match self.peek() {
                Some('i') => flags.icase = !negating,
                Some('m') => flags.dotall = !negating,
                Some('x') => flags.extended = !negating,
                // `(?a)`, `(?u)` and `(?d)` cannot be turned off, only
                // replaced: `(?-a)` is an undefined option in Ruby.
                Some('a') if !negating => flags.range = CharRange::Ascii,
                Some('u') if !negating => flags.range = CharRange::Unicode,
                Some('d') if !negating => flags.range = CharRange::Default,
                Some('-') if !negating => negating = true,
                Some(':') => {
                    self.pos += 1;
                    self.flags = flags;
                    let body = self.grouped_alternation()?;
                    self.flags = saved;
                    return Ok(Ast::Group {
                        index: None,
                        body: Box::new(body),
                    });
                }
                Some(')') => {
                    self.pos += 1;
                    // `(?-)` and `(?-:)` name no flag at all, which Ruby
                    // rejects rather than treating as a no-op.
                    if !seen {
                        return Err(Error::Syntax("undefined group option".into()));
                    }
                    self.flags = flags;
                    return Ok(Ast::Empty);
                }
                _ => return Err(Error::Syntax("undefined group option".into())),
            }
            seen = true;
            self.pos += 1;
        }
    }

    /// The body of any `(...)`, up to and including its `)`.
    fn grouped_alternation(&mut self) -> Result<Ast, Error> {
        let body = self.alternation()?;
        if !self.eat(')') {
            return Err(Error::Syntax(
                "end pattern with unmatched parenthesis".into(),
            ));
        }
        Ok(body)
    }

    fn escape(&mut self) -> Result<Ast, Error> {
        let Some(c) = self.next() else {
            return Err(Error::Syntax("too short escape sequence".into()));
        };
        Ok(match c {
            'A' => Ast::Anchor(Anchor::TextStart),
            'z' => Ast::Anchor(Anchor::TextEnd),
            'Z' => Ast::Anchor(Anchor::TextEndNewline),
            'b' => Ast::Anchor(Anchor::WordBoundary),
            'B' => Ast::Anchor(Anchor::NotWordBoundary),
            'G' => Ast::Anchor(Anchor::MatchStart),
            'w' | 'W' | 'd' | 'D' | 's' | 'S' | 'h' | 'H' => Ast::Class(self.shorthand_class(c)),
            'k' => return self.named_backref(),
            'g' => return self.subexpression_call(),
            'K' => Ast::Keep,
            'R' => self.line_break(),
            'X' => Ast::Grapheme,
            'p' | 'P' => match self.property(c == 'P')? {
                Some((item, negated)) => Ast::Class(Class {
                    negated,
                    icase: self.flags.icase,
                    items: vec![item],
                    intersect: Vec::new(),
                }),
                None => self.literal(c),
            },
            '1'..='9' => {
                // A backreference, reading as many digits as name a group that
                // already exists. A single digit may still be a *forward*
                // reference — `/\1()/` is legal — but a two-digit one may not:
                // `\10` with fewer than ten groups behind it is the octal
                // escape `\010`, which is why `/\10()()()()()()()()()()/`
                // matches a backspace. "disallows forward references >= 10".
                let start = self.pos - 1;
                // Read the whole digit run before deciding what it is. A run
                // that names an existing group is a backreference; a single
                // digit is one too, even pointing forwards, because `/\1()/`
                // is legal. Anything else is an octal escape, which is why
                // `/\10()()()()()()()()()()/` matches a backspace rather than
                // referring to the tenth group.
                let mut digits = String::from(c);
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    digits.push(self.next().expect("just peeked"));
                }
                let value = digits.parse::<usize>().unwrap_or(usize::MAX);
                // `\8` and `\9` start no octal escape, so past the groups
                // they are the digit itself: `/\99999/` matches `"99999"`.
                if value > self.groups && digits.len() > 1 && matches!(c, '8' | '9') {
                    self.pos = start + 1;
                    return Ok(self.literal(c));
                }
                if value > self.groups && digits.len() > 1 {
                    self.pos = start;
                    let ch = self.octal_escape()?;
                    return Ok(self.literal(ch));
                }
                self.numeric_backref = true;
                Ast::Backref {
                    group: value,
                    icase: self.flags.icase,
                }
            }
            _ => {
                let ch = self.escaped_char(c)?;
                self.literal(ch)
            }
        })
    }

    /// `\g<name>` or `\g'name'`: kept as written until the whole pattern is
    /// read, since it may call a group defined after it.
    fn subexpression_call(&mut self) -> Result<Ast, Error> {
        let close = match self.next() {
            Some('<') => '>',
            Some('\'') => '\'',
            _ => return Err(Error::Syntax("invalid group name <>".into())),
        };
        let mut name = String::new();
        loop {
            match self.next() {
                Some(c) if c == close => break,
                Some(c) => name.push(c),
                None => return Err(Error::Syntax("invalid group name <>".into())),
            }
        }
        // `\g<0>` is the whole pattern, which a name does not renumber, so a
        // pattern with names may call it: `/(?<a>x)\g<0>?/`. Measured.
        if name.parse::<usize>().is_ok_and(|n| n != 0) {
            self.numeric_backref = true;
        }
        // `\g<-1>` calls the group opened last, `\g<+1>` the next one to
        // open; both are kept as the absolute number they come to.
        if let Some(sign @ ('-' | '+')) = name.chars().next() {
            let n = name[1..]
                .parse::<usize>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| Error::Syntax(format!("invalid group name <{name}>")))?;
            let group = if sign == '-' {
                if n > self.groups {
                    return Err(Error::Syntax("invalid backref number/name".into()));
                }
                self.groups + 1 - n
            } else {
                self.groups + n
            };
            self.numeric_backref = true;
            return Ok(Ast::Call(group.to_string()));
        }
        Ok(Ast::Call(name))
    }

    fn named_backref(&mut self) -> Result<Ast, Error> {
        let close = match self.next() {
            Some('<') => '>',
            Some('\'') => '\'',
            _ => return Err(Error::Syntax("invalid backref name".into())),
        };
        let name = self
            .delimited_name(close)
            .ok_or_else(|| Error::Syntax("invalid backref name".into()))?;
        Ok(match self.leveled_reference(&name)? {
            (group, None) => Ast::Backref {
                group,
                icase: self.flags.icase,
            },
            (group, Some(level)) => Ast::LevelBackref {
                group,
                icase: self.flags.icase,
                level,
            },
        })
    }

    /// `\R`: a CRLF pair, or any one line break, taken atomically —
    /// `/\R\n/` does not match `"\r\n"`. Past ASCII the breaks are
    /// Unicode's, which only a Unicode pattern can name.
    fn line_break(&self) -> Ast {
        let mut items = vec![ClassItem::Range('\n', '\r')];
        if self.encoding == Encoding::Utf8 {
            items.push(ClassItem::Char('\u{85}'));
            items.push(ClassItem::Range('\u{2028}', '\u{2029}'));
        }
        let pair = Ast::Concat(vec![
            Ast::Literal {
                ch: '\r',
                icase: false,
            },
            Ast::Literal {
                ch: '\n',
                icase: false,
            },
        ]);
        let single = Ast::Class(Class {
            negated: false,
            icase: false,
            items,
            intersect: Vec::new(),
        });
        Ast::Atomic(Box::new(Ast::Alt(vec![pair, single])))
    }

    /// A shorthand or POSIX bracket as `(?a)` or `(?u)` reads it. Under `(?a)`
    /// a bracket is cut down to ASCII before it is negated, so `[[:^alpha:]]`
    /// still matches `あ`; under `(?u)` the shorthands read as the brackets
    /// of the same name. `\h` is ASCII under every option.
    fn ranged(&self, item: ClassItem) -> ClassItem {
        match (self.flags.range, item) {
            (CharRange::Ascii, ClassItem::Posix(kind, negated)) => {
                let ascii = Class {
                    negated: false,
                    icase: false,
                    items: vec![ClassItem::Posix(Posix::Ascii, false)],
                    intersect: Vec::new(),
                };
                ClassItem::Nested(Box::new(Class {
                    negated,
                    icase: self.flags.icase,
                    items: vec![ClassItem::Posix(kind, false)],
                    intersect: vec![ascii],
                }))
            }
            (CharRange::Unicode, ClassItem::Perl(kind, negated)) => match kind {
                Perl::Word => ClassItem::Posix(Posix::Word, negated),
                Perl::Digit => ClassItem::Posix(Posix::Digit, negated),
                Perl::Space => ClassItem::Posix(Posix::Space, negated),
                Perl::Hex => ClassItem::Perl(kind, negated),
            },
            (_, item) => item,
        }
    }

    fn shorthand_class(&self, c: char) -> Class {
        let (kind, negated) = match c {
            'w' => (Perl::Word, false),
            'W' => (Perl::Word, true),
            'd' => (Perl::Digit, false),
            'D' => (Perl::Digit, true),
            's' => (Perl::Space, false),
            'S' => (Perl::Space, true),
            'h' => (Perl::Hex, false),
            _ => (Perl::Hex, true),
        };
        Class {
            negated: false,
            icase: self.flags.icase,
            items: vec![self.ranged(ClassItem::Perl(kind, negated))],
            intersect: Vec::new(),
        }
    }

    /// One `[...]`. `/x` does not apply inside, and neither does `#`-comment
    /// skipping — `character_classes_spec.rb` checks both.
    fn bracket_class(&mut self) -> Result<Class, Error> {
        let negated = self.eat('^');
        let mut items: Vec<ClassItem> = Vec::new();
        let mut intersect: Vec<Class> = Vec::new();
        let mut first = true;

        loop {
            let Some(c) = self.peek() else {
                return Err(Error::Syntax("premature end of char-class".into()));
            };
            // A `]` in first position is a literal `]`.
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;

            // `&&` splits the class into intersected halves.
            if c == '&' && self.peek_at(1) == Some('&') {
                self.pos += 2;
                let rest = self.bracket_class_rest()?;
                intersect.push(rest);
                break;
            }

            if c == '[' && self.peek_at(1) == Some(':') {
                items.push(self.posix_class()?);
                self.no_range_from_a_class()?;
                continue;
            }
            if c == '[' {
                self.pos += 1;
                let nested = self.bracket_class()?;
                items.push(ClassItem::Nested(Box::new(nested)));
                continue;
            }

            let low = self.class_atom()?;
            let ClassAtom::Char(low_ch) = low else {
                let ClassAtom::Item(item) = low else {
                    unreachable!("class_atom answers one of the two")
                };
                items.push(item);
                self.no_range_from_a_class()?;
                continue;
            };

            // A `-` that is not last and not before `]` makes a range.
            if self.peek() == Some('-')
                && self.peek_at(1).is_some_and(|c| c != ']')
                && self.peek_at(1).is_some()
            {
                self.pos += 1;
                // A range cannot end at a class: `[a-\w]`, `[a-[:digit:]]`.
                let end_of_range = || Error::Syntax("char-class value at end of range".into());
                if self.peek() == Some('[') && self.peek_at(1) == Some(':') {
                    return Err(end_of_range());
                }
                let ClassAtom::Char(high) = self.class_atom()? else {
                    return Err(end_of_range());
                };
                if low_ch > high {
                    return Err(Error::Syntax("empty range in char class".into()));
                }
                items.push(ClassItem::Range(low_ch, high));
                continue;
            }
            items.push(ClassItem::Char(low_ch));
        }

        Ok(Class {
            negated,
            icase: self.flags.icase,
            items,
            intersect,
        })
    }

    /// Nor can a range start at one: after `\w`, `[:alpha:]` or `\p{L}`, a
    /// `-` is literal only before the `]` or a `&&`. `[\w-z]` is an error,
    /// `[\w-]` the two. Measured on CRuby 4.0.9.
    fn no_range_from_a_class(&self) -> Result<(), Error> {
        let ranged = self.peek() == Some('-')
            && !matches!(self.peek_at(1), None | Some(']'))
            && !(self.peek_at(1) == Some('&') && self.peek_at(2) == Some('&'));
        if ranged {
            return Err(Error::Syntax(
                "unmatched range specifier in char-class".into(),
            ));
        }
        Ok(())
    }

    /// The half of `[a&&b]` after the `&&`, which runs to the same `]`.
    fn bracket_class_rest(&mut self) -> Result<Class, Error> {
        // Reuse the class parser by pretending the `&&` opened a new class:
        // it consumes up to the shared `]`, which is exactly right.
        self.bracket_class()
    }

    /// `\p{name}`, `\p{^name}` or `\P{name}`, after its `p` or `P`: the
    /// property, unnegated, and whether the escape negates it. `None` when no
    /// brace follows, and Onigmo reads the letter itself (with a warning).
    ///
    /// The name ends at `}`, and `(`, `)`, `{` or `|` before one end it as
    /// invalid. The message names what was read, and an unterminated name
    /// minus its last character, as Onigmo's does: `\p{Alpha` reports
    /// `{Alph}`. A UTF-8 pattern reads every property CRuby measured; any
    /// other only the POSIX names, in any case. Measured on CRuby 4.0.9.
    fn property(&mut self, upper: bool) -> Result<Option<(ClassItem, bool)>, Error> {
        if !self.eat('{') {
            return Ok(None);
        }
        let negated = upper != self.eat('^');
        let mut name = String::new();
        let invalid =
            |name: &str| Error::Syntax(format!("invalid character property name {{{name}}}"));
        loop {
            match self.next() {
                Some('}') => break,
                Some('(' | ')' | '{' | '|') => return Err(invalid(&name)),
                Some(c) => name.push(c),
                None => {
                    name.pop();
                    return Err(invalid(&name));
                }
            }
        }
        if self.encoding == Encoding::Utf8 {
            return match crate::property::lookup(&name) {
                Some(property) => Ok(Some((ClassItem::Property(property, false), negated))),
                None => Err(invalid(&name)),
            };
        }
        let lower = name.to_ascii_lowercase();
        if let Some(kind) = posix_kind(&lower) {
            return Ok(Some((ClassItem::Posix(kind, false), negated)));
        }
        // EUC-JP and Shift_JIS have six scripts of their own, as ranges of
        // their own codes, that no table here holds yet.
        if matches!(self.encoding, Encoding::EucJp | Encoding::ShiftJis)
            && matches!(
                lower.as_str(),
                "hiragana" | "katakana" | "han" | "latin" | "greek" | "cyrillic"
            )
        {
            return Err(Error::Unsupported(
                "a \\p{} script property in an EUC-JP or Shift_JIS pattern",
            ));
        }
        Err(invalid(&name))
    }

    fn posix_class(&mut self) -> Result<ClassItem, Error> {
        let start = self.pos;
        self.pos += 2; // `[:`
        let negated = self.eat('^');
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c == ':' {
                break;
            }
            name.push(c);
            self.pos += 1;
        }
        if !(self.eat(':') && self.eat(']')) {
            self.pos = start;
            return Err(Error::Syntax("invalid POSIX bracket type".into()));
        }
        let kind =
            posix_kind(&name).ok_or_else(|| Error::Syntax("invalid POSIX bracket type".into()))?;
        Ok(self.ranged(ClassItem::Posix(kind, negated)))
    }

    /// One element inside a bracket class: either a plain character, which may
    /// go on to be the low end of a range, or a shorthand, which may not.
    fn class_atom(&mut self) -> Result<ClassAtom, Error> {
        let Some(c) = self.next() else {
            return Err(Error::Syntax("premature end of char-class".into()));
        };
        if c != '\\' {
            return Ok(ClassAtom::Char(c));
        }
        let Some(e) = self.next() else {
            return Err(Error::Syntax("too short escape sequence".into()));
        };
        Ok(match e {
            'w' | 'W' | 'd' | 'D' | 's' | 'S' | 'h' | 'H' => {
                let (kind, negated) = match e {
                    'w' => (Perl::Word, false),
                    'W' => (Perl::Word, true),
                    'd' => (Perl::Digit, false),
                    'D' => (Perl::Digit, true),
                    's' => (Perl::Space, false),
                    'S' => (Perl::Space, true),
                    'h' => (Perl::Hex, false),
                    _ => (Perl::Hex, true),
                };
                ClassAtom::Item(self.ranged(ClassItem::Perl(kind, negated)))
            }
            'p' | 'P' => match self.property(e == 'P')? {
                Some((ClassItem::Property(property, _), negated)) => {
                    ClassAtom::Item(ClassItem::Property(property, negated))
                }
                Some((ClassItem::Posix(kind, _), negated)) => {
                    ClassAtom::Item(ClassItem::Posix(kind, negated))
                }
                Some(_) => unreachable!("a property is one of the two"),
                None => ClassAtom::Char(e),
            },
            _ => ClassAtom::Char(self.escaped_char(e)?),
        })
    }

    /// Up to three octal digits, from the current position.
    fn octal_escape(&mut self) -> Result<char, Error> {
        let mut value: u32 = 0;
        let mut seen = 0;
        while seen < 3 && self.peek().is_some_and(|c| ('0'..='7').contains(&c)) {
            value = value * 8 + (self.next().expect("just peeked") as u32 - '0' as u32);
            seen += 1;
        }
        if value > 0x7f && self.encoding.multibyte_coded() {
            let lead = u8::try_from(value).map_err(|_| {
                Error::Unsupported("an octal escape past a byte in a multibyte pattern")
            })?;
            return self.multibyte_escape(lead);
        }
        char::from_u32(value).ok_or_else(|| Error::Syntax("invalid octal escape".into()))
    }

    /// The character an escaped byte past ASCII starts, in a multibyte
    /// encoding past UTF-8: the escapes after it supply the rest of its
    /// bytes, as Onigmo reads them — `\xa4\xa2` is `あ` in EUC-JP,
    /// `\244\242` too, and `\xa4` alone is too short. Measured on ruby
    /// 4.0.7, messages included.
    fn multibyte_escape(&mut self, lead: u8) -> Result<char, Error> {
        let invalid = || Error::Syntax("invalid multibyte escape".into());
        let mut bytes = vec![lead];
        while bytes.len() < self.encoding.lead_len(&bytes).ok_or_else(invalid)? {
            let byte = self
                .escaped_byte()
                .ok_or_else(|| Error::Syntax("too short escaped multibyte character".into()))?;
            bytes.push(byte);
        }
        if self.encoding.uncarried(&bytes) {
            return Err(Error::Unsupported(
                "a GB18030 four-byte code no character is assigned",
            ));
        }
        self.encoding.spelled_by(&bytes).ok_or_else(invalid)
    }

    /// One byte spelled as an escape, `\xHH` or `\NNN`, consumed; or `None`,
    /// with nothing consumed, when what follows is not one.
    fn escaped_byte(&mut self) -> Option<u8> {
        if self.peek() != Some('\\') {
            return None;
        }
        let save = self.pos;
        self.pos += 1;
        let (radix, max) = match self.peek() {
            Some('x') => {
                self.pos += 1;
                (16, 2)
            }
            Some('0'..='7') => (8, 3),
            _ => {
                self.pos = save;
                return None;
            }
        };
        let mut value: u32 = 0;
        let mut seen = 0;
        while seen < max {
            let Some(digit) = self.peek().and_then(|c| c.to_digit(radix)) else {
                break;
            };
            value = value * radix + digit;
            self.pos += 1;
            seen += 1;
        }
        match u8::try_from(value) {
            Ok(byte) if seen > 0 => Some(byte),
            _ => {
                self.pos = save;
                None
            }
        }
    }

    /// The character an escape stands for, for the escapes that stand for one.
    fn escaped_char(&mut self, c: char) -> Result<char, Error> {
        let ch = match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'f' => '\x0c',
            'v' => '\x0b',
            'a' => '\x07',
            'e' => '\x1b',
            // Only reachable from inside a bracket class: outside one, `\b`
            // is a word boundary and never gets this far.
            'b' => '\x08',
            // An octal escape, up to three digits. Only reachable from inside a
            // bracket class for `1`-`7`: outside one those are backreferences
            // and never get this far, which is the whole difference between
            // `/(a)\1/` and `/[\1]/` — the second matches U+0001, measured.
            // `\8` and `\9` are not octal digits and stay literal, also
            // measured.
            '0'..='7' => {
                self.pos -= 1;
                self.octal_escape()?
            }
            'x' => {
                if self.eat('{') {
                    let mut hex = String::new();
                    while self.peek().is_some_and(|c| c != '}') {
                        hex.push(self.next().expect("just peeked"));
                    }
                    if !self.eat('}') {
                        return Err(Error::Syntax("invalid hex escape".into()));
                    }
                    let n = u32::from_str_radix(hex.trim(), 16)
                        .map_err(|_| Error::Syntax("invalid hex escape".into()))?;
                    char::from_u32(n).ok_or_else(|| Error::Syntax("invalid hex escape".into()))?
                } else {
                    let mut hex = String::new();
                    while hex.len() < 2 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                        hex.push(self.next().expect("just peeked"));
                    }
                    if hex.is_empty() {
                        return Err(Error::Syntax("invalid hex escape".into()));
                    }
                    let n = u32::from_str_radix(&hex, 16)
                        .map_err(|_| Error::Syntax("invalid hex escape".into()))?;
                    // A byte above ASCII is not a codepoint. In a binary or
                    // single-byte pattern it is the byte, which the matcher
                    // carries as the `char` with the same number; in EUC-JP
                    // and the other multibyte encodings it leads a character
                    // the escapes after it
                    // finish; in UTF-8 it would too, which waits for the rest
                    // of the Onigmo dialect.
                    match self.encoding {
                        _ if n <= 0x7f => char::from_u32(n)
                            .ok_or_else(|| Error::Syntax("invalid hex escape".into()))?,
                        Encoding::Utf8 => {
                            return Err(Error::Unsupported("a non-ASCII \\x byte escape"));
                        }
                        Encoding::Binary | Encoding::Single(_) => char::from(n as u8),
                        _ => self.multibyte_escape(n as u8)?,
                    }
                }
            }
            'u' => {
                if self.eat('{') {
                    let mut hex = String::new();
                    while self.peek().is_some_and(|c| c != '}') {
                        hex.push(self.next().expect("just peeked"));
                    }
                    if !self.eat('}') {
                        return Err(Error::Syntax("invalid unicode escape".into()));
                    }
                    // `\u{1 2 3}` is a list; only the single form is wanted
                    // where one character is expected.
                    if hex.trim().contains(char::is_whitespace) {
                        return Err(Error::Unsupported("a multi-codepoint \\u{} escape"));
                    }
                    let n = u32::from_str_radix(hex.trim(), 16)
                        .map_err(|_| Error::Syntax("invalid unicode escape".into()))?;
                    char::from_u32(n)
                        .ok_or_else(|| Error::Syntax("invalid unicode escape".into()))?
                } else {
                    let mut hex = String::new();
                    while hex.len() < 4 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                        hex.push(self.next().expect("just peeked"));
                    }
                    if hex.len() != 4 {
                        return Err(Error::Syntax("invalid unicode escape".into()));
                    }
                    let n = u32::from_str_radix(&hex, 16)
                        .map_err(|_| Error::Syntax("invalid unicode escape".into()))?;
                    char::from_u32(n)
                        .ok_or_else(|| Error::Syntax("invalid unicode escape".into()))?
                }
            }
            'c' | 'C' => {
                // `\cX` and `\C-X` are the same control character.
                if c == 'C' && !self.eat('-') {
                    return Err(Error::Syntax("invalid control-code syntax".into()));
                }
                let mut target = self
                    .next()
                    .ok_or_else(|| Error::Syntax("invalid control-code syntax".into()))?;
                if target == '\\' {
                    let escaped = self
                        .next()
                        .ok_or_else(|| Error::Syntax("invalid control-code syntax".into()))?;
                    target = self.escaped_char(escaped)?;
                }
                char::from_u32((target as u32) & 0x9f)
                    .ok_or_else(|| Error::Syntax("invalid control-code syntax".into()))?
            }
            // "allows any character to be escaped": an escape with no special
            // meaning is the character itself.
            other => other,
        };
        // A codepoint past ASCII names a Unicode character, which a pattern
        // in any other encoding has no way to hold: CRuby's "UTF-8 character
        // in non UTF-8 regexp", refused here rather than spelled.
        if c == 'u' && !ch.is_ascii() && self.encoding != Encoding::Utf8 {
            return Err(Error::Unsupported(
                "a non-ASCII \\u escape in a pattern that is not UTF-8",
            ));
        }
        Ok(ch)
    }
}

enum ClassAtom {
    Char(char),
    Item(ClassItem),
}

/// A POSIX bracket by name, as `[[:name:]]` spells it.
fn posix_kind(name: &str) -> Option<Posix> {
    Some(match name {
        "alpha" => Posix::Alpha,
        "digit" => Posix::Digit,
        "alnum" => Posix::Alnum,
        "upper" => Posix::Upper,
        "lower" => Posix::Lower,
        "space" => Posix::Space,
        "blank" => Posix::Blank,
        "punct" => Posix::Punct,
        "print" => Posix::Print,
        "graph" => Posix::Graph,
        "cntrl" => Posix::Cntrl,
        "xdigit" => Posix::XDigit,
        "word" => Posix::Word,
        "ascii" => Posix::Ascii,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ast(source: &str) -> Ast {
        parse(source, Flags::default(), Encoding::Utf8)
            .expect("parses")
            .ast
    }

    #[test]
    fn literals_concatenate() {
        assert_eq!(
            ast("ab"),
            Ast::Concat(vec![
                Ast::Literal {
                    ch: 'a',
                    icase: false
                },
                Ast::Literal {
                    ch: 'b',
                    icase: false
                },
            ])
        );
    }

    #[test]
    fn caret_is_a_line_anchor_not_a_string_anchor() {
        // The difference from Rust's dialect, asserted rather than assumed.
        assert_eq!(ast("^"), Ast::Anchor(Anchor::LineStart));
        assert_eq!(ast("\\A"), Ast::Anchor(Anchor::TextStart));
    }

    #[test]
    fn inline_flags_apply_to_what_follows() {
        let Ast::Concat(items) = ast("a(?i)b") else {
            panic!("expected a concatenation")
        };
        assert_eq!(
            items[0],
            Ast::Literal {
                ch: 'a',
                icase: false
            }
        );
        assert_eq!(
            items[2],
            Ast::Literal {
                ch: 'b',
                icase: true
            }
        );
    }

    #[test]
    fn scoped_flags_stop_at_the_closing_paren() {
        let Ast::Concat(items) = ast("(?i:a)b") else {
            panic!("expected a concatenation")
        };
        assert_eq!(
            items[1],
            Ast::Literal {
                ch: 'b',
                icase: false
            }
        );
    }

    #[test]
    fn trailing_question_after_bounded_repeat_is_a_second_quantifier() {
        // `a{2}?` is `(a{2})?`, not a lazy `a{2}`.
        let Ast::Repeat {
            body,
            min,
            max,
            greedy,
        } = ast("a{2}?")
        else {
            panic!("expected a repeat")
        };
        assert_eq!((min, max, greedy), (0, Some(1), true));
        assert!(matches!(
            *body,
            Ast::Repeat {
                min: 2,
                max: Some(2),
                ..
            }
        ));
    }

    #[test]
    fn unmatched_paren_is_a_syntax_error() {
        assert!(matches!(
            parse("(", Flags::default(), Encoding::Utf8),
            Err(Error::Syntax(_))
        ));
        assert!(matches!(
            parse(")", Flags::default(), Encoding::Utf8),
            Err(Error::Syntax(_))
        ));
    }

    #[test]
    fn onigmo_only_constructs_refuse_rather_than_guess() {
        for source in ["(?{a})", "(a)(?(<1+1>)a|b)"] {
            assert!(
                matches!(
                    parse(source, Flags::default(), Encoding::Utf8),
                    Err(Error::Unsupported(_))
                ),
                "{source} should refuse"
            );
        }
    }

    /// What `\p{}` refuses, and the name each message carries, as CRuby
    /// 4.0.9 reports it.
    #[test]
    fn a_property_name_is_checked_as_onigmo_does() {
        let message = |source: &str, encoding| match parse(source, Flags::default(), encoding) {
            Err(Error::Syntax(message)) => message,
            Err(other) => panic!("{source}: {other}"),
            Ok(_) => panic!("{source} should not parse"),
        };
        let invalid = |name: &str| format!("invalid character property name {{{name}}}");
        assert_eq!(message("\\p{Foo}", Encoding::Utf8), invalid("Foo"));
        assert_eq!(message("\\p{", Encoding::Utf8), invalid(""));
        assert_eq!(message("\\p{^}", Encoding::Utf8), invalid(""));
        assert_eq!(message("\\p{Alpha", Encoding::Utf8), invalid("Alph"));
        assert_eq!(message("\\p{L(}", Encoding::Utf8), invalid("L"));
        assert_eq!(message("\\p{ ^L}", Encoding::Utf8), invalid(" ^L"));
        assert_eq!(message("[\\p{é}]", Encoding::Utf8), invalid("é"));
        assert_eq!(message("\\p{L}", Encoding::Binary), invalid("L"));
        assert_eq!(message("\\p{al_pha}", Encoding::Binary), invalid("al_pha"));
        assert!(parse("\\p{Alpha}", Flags::default(), Encoding::Binary).is_ok());
        assert!(parse("\\p{^ASCII}", Flags::default(), Encoding::Binary).is_ok());
        assert!(matches!(
            parse("\\p{Hiragana}", Flags::default(), Encoding::EucJp),
            Err(Error::Unsupported(_))
        ));
        // With no brace, `\p` is the letter.
        assert_eq!(
            parse("\\pL", Flags::default(), Encoding::Utf8)
                .map(|p| p.ast)
                .ok(),
            parse("pL", Flags::default(), Encoding::Utf8)
                .map(|p| p.ast)
                .ok()
        );
    }

    #[test]
    fn group_names_reject_a_leading_digit() {
        assert!(parse("(?<1a>x)", Flags::default(), Encoding::Utf8).is_err());
        assert!(parse("(?<a1>x)", Flags::default(), Encoding::Utf8).is_ok());
    }

    #[test]
    fn extended_mode_drops_whitespace_and_comments() {
        let flags = Flags {
            extended: true,
            ..Flags::default()
        };
        let parsed = parse("a b # trailing\nc", flags, Encoding::Utf8).expect("parses");
        let Ast::Concat(items) = parsed.ast else {
            panic!("expected a concatenation")
        };
        assert_eq!(items.len(), 3);
    }
}
