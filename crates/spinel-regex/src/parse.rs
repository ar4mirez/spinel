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

use crate::{Error, Flags};
use std::collections::HashMap;

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

pub fn parse(source: &str, flags: Flags) -> Result<Parsed, Error> {
    let mut parser = Parser {
        chars: source.chars().collect(),
        pos: 0,
        flags,
        groups: 0,
        names: Vec::new(),
        name_index: HashMap::new(),
    };
    let ast = parser.alternation()?;
    if parser.pos < parser.chars.len() {
        // The only way to stop early is an unbalanced `)`.
        return Err(Error::Syntax("unmatched close parenthesis".into()));
    }
    Ok(Parsed {
        ast,
        groups: parser.groups,
        names: parser.names,
    })
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    flags: Flags,
    groups: usize,
    names: Vec<(String, usize)>,
    name_index: HashMap<String, usize>,
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
            Some('~') => Err(Error::Unsupported("the (?~) absent operator")),
            Some('(') => Err(Error::Unsupported("a conditional group (?(...)...)")),
            Some('{') => Err(Error::Unsupported("an embedded code block (?{...})")),
            _ => self.inline_flags(),
        }
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
            'g' => return Err(Error::Unsupported("a \\g<> subexpression call")),
            'K' => return Err(Error::Unsupported("the \\K keep operator")),
            'R' => return Err(Error::Unsupported("the \\R line break escape")),
            'X' => return Err(Error::Unsupported("the \\X grapheme cluster escape")),
            'p' | 'P' => return Err(Error::Unsupported("a \\p{} unicode property")),
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
                if value > self.groups && digits.len() > 1 {
                    self.pos = start;
                    let ch = self.octal_escape()?;
                    return Ok(self.literal(ch));
                }
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

    fn named_backref(&mut self) -> Result<Ast, Error> {
        let close = match self.next() {
            Some('<') => '>',
            Some('\'') => '\'',
            _ => return Err(Error::Syntax("invalid backref name".into())),
        };
        let mut name = String::new();
        loop {
            match self.next() {
                Some(c) if c == close => break,
                Some(c) => name.push(c),
                None => return Err(Error::Syntax("invalid backref name".into())),
            }
        }
        // `\k<name+1>` asks for a capture at a recursion level, which needs the
        // subexpression-call machinery this engine does not have.
        if name.contains('+') || name.contains('-') {
            return Err(Error::Unsupported("a \\k<> level specifier"));
        }
        let group = if let Ok(n) = name.parse::<usize>() {
            n
        } else {
            *self
                .name_index
                .get(&name)
                .ok_or_else(|| Error::Syntax(format!("unknown group name: {name}")))?
        };
        Ok(Ast::Backref {
            group,
            icase: self.flags.icase,
        })
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
            items: vec![ClassItem::Perl(kind, negated)],
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
                continue;
            };

            // A `-` that is not last and not before `]` makes a range.
            if self.peek() == Some('-')
                && self.peek_at(1).is_some_and(|c| c != ']')
                && self.peek_at(1).is_some()
            {
                self.pos += 1;
                match self.class_atom()? {
                    ClassAtom::Char(high) => {
                        if low_ch > high {
                            return Err(Error::Syntax("empty range in char class".into()));
                        }
                        items.push(ClassItem::Range(low_ch, high));
                    }
                    // `[\w-z]` is `\w`, `-`, `z` in Ruby, not a range.
                    ClassAtom::Item(item) => {
                        items.push(ClassItem::Char(low_ch));
                        items.push(ClassItem::Char('-'));
                        items.push(item);
                    }
                }
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

    /// The half of `[a&&b]` after the `&&`, which runs to the same `]`.
    fn bracket_class_rest(&mut self) -> Result<Class, Error> {
        // Reuse the class parser by pretending the `&&` opened a new class:
        // it consumes up to the shared `]`, which is exactly right.
        self.bracket_class()
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
        let kind = match name.as_str() {
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
            _ => return Err(Error::Syntax("invalid POSIX bracket type".into())),
        };
        Ok(ClassItem::Posix(kind, negated))
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
                ClassAtom::Item(ClassItem::Perl(kind, negated))
            }
            'p' | 'P' => return Err(Error::Unsupported("a \\p{} unicode property")),
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
        char::from_u32(value).ok_or_else(|| Error::Syntax("invalid octal escape".into()))
    }

    /// The character an escape stands for, for the escapes that stand for one.
    fn escaped_char(&mut self, c: char) -> Result<char, Error> {
        Ok(match c {
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
            '0' => {
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
                    // A byte above ASCII is not a codepoint: it makes the
                    // pattern ASCII-8BIT, and matching a binary pattern
                    // against a UTF-8 subject is the Encoding slice's problem.
                    if n > 0x7f {
                        return Err(Error::Unsupported("a non-ASCII \\x byte escape"));
                    }
                    char::from_u32(n).ok_or_else(|| Error::Syntax("invalid hex escape".into()))?
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
        })
    }
}

enum ClassAtom {
    Char(char),
    Item(ClassItem),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ast(source: &str) -> Ast {
        parse(source, Flags::default()).expect("parses").ast
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
            parse("(", Flags::default()),
            Err(Error::Syntax(_))
        ));
        assert!(matches!(
            parse(")", Flags::default()),
            Err(Error::Syntax(_))
        ));
    }

    #[test]
    fn onigmo_only_constructs_refuse_rather_than_guess() {
        for source in ["(?~foo)", "\\g<name>", "a\\Kb", "\\R", "\\p{Alpha}"] {
            assert!(
                matches!(parse(source, Flags::default()), Err(Error::Unsupported(_))),
                "{source} should refuse"
            );
        }
    }

    #[test]
    fn group_names_reject_a_leading_digit() {
        assert!(parse("(?<1a>x)", Flags::default()).is_err());
        assert!(parse("(?<a1>x)", Flags::default()).is_ok());
    }

    #[test]
    fn extended_mode_drops_whitespace_and_comments() {
        let flags = Flags {
            extended: true,
            ..Flags::default()
        };
        let parsed = parse("a b # trailing\nc", flags).expect("parses");
        let Ast::Concat(items) = parsed.ast else {
            panic!("expected a concatenation")
        };
        assert_eq!(items.len(), 3);
    }
}
