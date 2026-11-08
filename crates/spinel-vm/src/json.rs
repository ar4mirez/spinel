//! The JSON parser behind `JSON::Ext::ParserConfig#parse` (#55).
//!
//! The json gem's Ruby half is upstream's, vendored in `stdlib/json/`; its C
//! parser, `ext/json/ext/parser/parser.c` at json 2.18.0, is this file. It is
//! the same grammar with the same extensions (comments, `NaN` and the
//! infinities under `allow_nan`, trailing commas, raw control characters),
//! the same limits and the same error messages, down to the quoted fragment
//! and the line and column the C parser reports, quirks included: the column
//! of a newline is 0, and the line does not count the newline that ends the
//! one before the error's.
//!
//! The input is untrusted, so the parser never panics on it, uses no
//! `unsafe`, and does not recurse: containers are an explicit stack, so a
//! document nested a million deep with `max_nesting: false` costs memory in
//! proportion to its size and nothing more. Where the C parser reads one byte
//! past the end of its input (its strings are NUL-terminated), this one reads
//! a 0, which is what the C parser finds there.
//!
//! The parser builds nothing itself. A [`Sink`] makes each value as it
//! completes, in the order the C parser makes them, so `interp.rs` can build
//! Ruby objects and a test can build a tree of its own.

use std::collections::HashMap;

/// What a duplicated key in an object does: json 2.18's deprecation warning
/// by default, nothing under `allow_duplicate_key: true`, a `ParserError`
/// under `allow_duplicate_key: false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplicates {
    Warn,
    Ignore,
    Raise,
}

/// `JSON::Ext::ParserConfig`'s options that the grammar reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The deepest an array or object may nest; 0 checks nothing. An `int`
    /// in the C parser, so a negative one refuses any nesting at all.
    pub max_nesting: i64,
    pub allow_nan: bool,
    pub allow_trailing_comma: bool,
    pub allow_control_characters: bool,
    pub duplicates: Duplicates,
    /// Hand every object to the sink with all its pairs, duplicates and
    /// all, and leave duplicates to it: what the Ruby half needs when an
    /// `on_load` proc has to see every value the C parser would show it.
    pub raw_objects: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            max_nesting: 100,
            allow_nan: false,
            allow_trailing_comma: false,
            allow_control_characters: false,
            duplicates: Duplicates::Warn,
            raw_objects: false,
        }
    }
}

/// The literals the grammar has besides numbers and strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Literal {
    Null,
    True,
    False,
    NaN,
    Infinity,
    MinusInfinity,
}

/// Where the values go. Each method makes one value and answers it; the
/// parser keeps what it is answered until the container it belongs to is
/// complete.
pub trait Sink {
    type Value: Copy;

    fn literal(&mut self, which: Literal) -> Self::Value;
    /// An integer: `small` when it has fewer than 18 digits, `text` (with its
    /// sign) always, for one that is not.
    fn integer(&mut self, small: Option<i64>, text: &[u8]) -> Self::Value;
    /// A number with a fraction or an exponent: `value` as the C parser
    /// rounds it, and `text` for a `decimal_class`.
    fn float(&mut self, value: f64, text: &[u8]) -> Self::Value;
    /// A string's bytes, unescaped. `key` when it names an object member.
    fn string(&mut self, bytes: &[u8], key: bool) -> Self::Value;
    fn array(&mut self, items: &[Self::Value]) -> Self::Value;
    /// An object, as keys and values alternating. Without `raw_objects` no
    /// key repeats: a repeated key keeps its first place and its last value,
    /// as `Hash#[]=` would leave it. `offset` is the byte offset of the `{`.
    fn object(&mut self, pairs: &[Self::Value], offset: usize) -> Self::Value;
}

/// Why a parse failed.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure<V> {
    /// A `JSON::ParserError`: the whole message, then the line and column
    /// also set on the exception.
    Syntax {
        message: Vec<u8>,
        line: i64,
        column: i64,
    },
    /// A `JSON::NestingError`, "nesting of N is too deep".
    Nesting { depth: i64 },
    /// A repeated key under `allow_duplicate_key: false`. The C parser
    /// formats this message with the key's `inspect` as its format string,
    /// so it is the Ruby half's to finish: the key, the fragment a `%s` in
    /// it would print, and where.
    Duplicate {
        key: V,
        fragment: Vec<u8>,
        line: i64,
        column: i64,
    },
}

/// A repeated key the default duplicate handling warns about.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning<V> {
    pub key: V,
    pub line: i64,
    pub column: i64,
}

/// What a parse produced: the document's value or why there is none, and
/// the warnings issued before it ended, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome<V> {
    pub result: Result<V, Failure<V>>,
    pub warnings: Vec<Warning<V>>,
}

/// Parse `input` as one JSON document.
pub fn parse<S: Sink>(input: &[u8], config: &Config, sink: &mut S) -> Outcome<S::Value> {
    let mut parser = Parser {
        input,
        cursor: 0,
        config,
        values: Vec::new(),
        keys: Vec::new(),
        key_bytes: Vec::new(),
        frames: Vec::new(),
        nesting: 0,
        warnings: Vec::new(),
    };
    let result = parser.document(sink);
    Outcome {
        result,
        warnings: parser.warnings,
    }
}

/// The byte offset of `at` as the C parser's `cursor_position` reports it:
/// the column counts back from `at` itself to the newline before it, and the
/// line counts the newlines before that one.
pub fn position(input: &[u8], at: usize) -> (i64, i64) {
    let byte = |i: usize| input.get(i).copied().unwrap_or(0);
    let mut cursor = at as i64;
    let mut column = 0;
    let mut line = 1;
    while cursor >= 0 {
        let b = byte(cursor as usize);
        cursor -= 1;
        if b == b'\n' {
            break;
        }
        column += 1;
    }
    while cursor >= 0 {
        if byte(cursor as usize) == b'\n' {
            line += 1;
        }
        cursor -= 1;
    }
    (line, column)
}

/// The quoted fragment of the input at `at` that an error message shows, as
/// `raise_parse_error` cuts it: up to 32 bytes, ending at a NUL, a newline or
/// blank, with a trailing partial character dropped (and, as the C code has
/// it, a trailing whole one too). `EOF` at the end of the input. When the
/// fragment would be empty the C code prints the rest of its input up to a
/// NUL instead, unquoted, and so does this.
pub fn fragment(input: &[u8], at: usize) -> Vec<u8> {
    const LIMIT: usize = 32;
    if at >= input.len() {
        return b"EOF".to_vec();
    }
    let rest = &input[at..];
    let mut len = 0;
    while len < LIMIT {
        match rest.get(len).copied().unwrap_or(0) {
            0 | b'\n' | b' ' | b'\t' | b'\r' => break,
            _ => len += 1,
        }
    }
    if len == 0 {
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        return rest[..end].to_vec();
    }
    // `buffer[0]` is the opening quote, so `buffer[len]` is the last byte
    // copied, and the quote stops the backing-up.
    let mut buffer = Vec::with_capacity(len + 2);
    buffer.push(b'\'');
    buffer.extend_from_slice(&rest[..len]);
    while (0x80..0xC0).contains(&buffer[len]) {
        len -= 1;
    }
    if buffer[len] >= 0xC0 {
        len -= 1;
    }
    buffer.truncate(len + 1);
    buffer.push(b'\'');
    buffer
}

enum Frame {
    /// An array whose items start at `base` in `values`.
    Array { base: usize },
    /// An object whose keys and values start at `base` in `values`, whose
    /// key bytes start at `keys` in `keys`, and whose `{` is at `offset`.
    Object {
        base: usize,
        keys: usize,
        offset: usize,
    },
}

struct Parser<'a, V> {
    input: &'a [u8],
    cursor: usize,
    config: &'a Config,
    values: Vec<V>,
    /// Each pending object key's bytes, as a range of `key_bytes`, for
    /// finding repeats without reading them back out of the sink's values.
    keys: Vec<(usize, usize)>,
    key_bytes: Vec<u8>,
    frames: Vec<Frame>,
    nesting: i64,
    warnings: Vec<Warning<V>>,
}

type Step<T, V> = Result<T, Failure<V>>;

impl<V: Copy> Parser<'_, V> {
    fn peek(&self) -> u8 {
        self.input.get(self.cursor).copied().unwrap_or(0)
    }

    fn at(&self, offset: usize) -> u8 {
        self.input.get(offset).copied().unwrap_or(0)
    }

    fn rest(&self) -> usize {
        self.input.len().saturating_sub(self.cursor)
    }

    fn eos(&self) -> bool {
        self.cursor >= self.input.len()
    }

    fn starts_with(&self, offset: usize, word: &[u8]) -> bool {
        self.input
            .get(offset..)
            .is_some_and(|rest| rest.starts_with(word))
    }

    /// A `ParserError` at `at`, its message `format` with `%s` the fragment
    /// there.
    fn error_at(&self, format: &str, at: usize) -> Failure<V> {
        let mut message = Vec::with_capacity(format.len() + 40);
        match format.split_once("%s") {
            Some((before, after)) => {
                message.extend_from_slice(before.as_bytes());
                message.extend_from_slice(&fragment(self.input, at));
                message.extend_from_slice(after.as_bytes());
            }
            None => message.extend_from_slice(format.as_bytes()),
        }
        let (line, column) = position(self.input, at);
        message.extend_from_slice(format!(" at line {line} column {column}").as_bytes());
        Failure::Syntax {
            message,
            line,
            column,
        }
    }

    fn error(&self, format: &str) -> Failure<V> {
        self.error_at(format, self.cursor)
    }

    fn document<S: Sink<Value = V>>(&mut self, sink: &mut S) -> Step<V, V> {
        let value = self.value(sink)?;
        self.whitespace()?;
        if !self.eos() {
            return Err(self.error("unexpected token at end of stream %s"));
        }
        Ok(value)
    }

    fn enter(&mut self) -> Step<(), V> {
        self.nesting += 1;
        if self.config.max_nesting != 0 && self.config.max_nesting < self.nesting {
            return Err(Failure::Nesting {
                depth: self.nesting,
            });
        }
        Ok(())
    }

    /// One whole value, containers included: the loop is `json_parse_any`
    /// with its recursion turned into `frames`.
    fn value<S: Sink<Value = V>>(&mut self, sink: &mut S) -> Step<V, V> {
        'value: loop {
            // Parse a value, or open a container and go round for its first.
            self.whitespace()?;
            let mut done = match self.peek() {
                b'[' => {
                    self.cursor += 1;
                    self.whitespace()?;
                    if self.peek() == b']' {
                        self.cursor += 1;
                        sink.array(&[])
                    } else {
                        self.enter()?;
                        self.frames.push(Frame::Array {
                            base: self.values.len(),
                        });
                        continue 'value;
                    }
                }
                b'{' => {
                    let offset = self.cursor;
                    self.cursor += 1;
                    self.whitespace()?;
                    if self.peek() == b'}' {
                        self.cursor += 1;
                        sink.object(&[], offset)
                    } else {
                        self.enter()?;
                        if self.peek() != b'"' {
                            return Err(self.error("expected object key, got %s"));
                        }
                        self.frames.push(Frame::Object {
                            base: self.values.len(),
                            keys: self.keys.len(),
                            offset,
                        });
                        let key = self.string(sink, true)?;
                        self.values.push(key);
                        self.whitespace()?;
                        if self.peek() != b':' {
                            return Err(self.error("expected ':' after object key"));
                        }
                        self.cursor += 1;
                        continue 'value;
                    }
                }
                _ => self.scalar(sink)?,
            };
            // A value is complete: give it to the container it is in, and
            // close every container it completes.
            loop {
                let Some(frame) = self.frames.last() else {
                    return Ok(done);
                };
                self.values.push(done);
                match *frame {
                    Frame::Array { base } => {
                        self.whitespace()?;
                        match self.peek() {
                            b',' => {
                                self.cursor += 1;
                                if self.config.allow_trailing_comma {
                                    self.whitespace()?;
                                    if self.peek() == b']' {
                                        self.cursor += 1;
                                        done = self.close_array(sink, base);
                                        continue;
                                    }
                                }
                                continue 'value;
                            }
                            b']' => {
                                self.cursor += 1;
                                done = self.close_array(sink, base);
                            }
                            _ => return Err(self.error("expected ',' or ']' after array value")),
                        }
                    }
                    Frame::Object { base, keys, offset } => {
                        self.whitespace()?;
                        match self.peek() {
                            b'}' => {
                                self.cursor += 1;
                                done = self.close_object(sink, base, keys, offset)?;
                            }
                            b',' => {
                                self.cursor += 1;
                                self.whitespace()?;
                                if self.config.allow_trailing_comma && self.peek() == b'}' {
                                    self.cursor += 1;
                                    done = self.close_object(sink, base, keys, offset)?;
                                    continue;
                                }
                                if self.peek() != b'"' {
                                    return Err(self.error("expected object key, got: %s"));
                                }
                                let key = self.string(sink, true)?;
                                self.values.push(key);
                                self.whitespace()?;
                                if self.peek() != b':' {
                                    return Err(
                                        self.error("expected ':' after object key, got: %s")
                                    );
                                }
                                self.cursor += 1;
                                continue 'value;
                            }
                            _ => {
                                return Err(
                                    self.error("expected ',' or '}' after object value, got: %s")
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Close the array on top of `frames`, whose items are `values[base..]`.
    fn close_array<S: Sink<Value = V>>(&mut self, sink: &mut S, base: usize) -> V {
        self.frames.pop();
        self.nesting -= 1;
        let array = sink.array(&self.values[base..]);
        self.values.truncate(base);
        array
    }

    fn close_object<S: Sink<Value = V>>(
        &mut self,
        sink: &mut S,
        base: usize,
        keys: usize,
        offset: usize,
    ) -> Step<V, V> {
        self.frames.pop();
        self.nesting -= 1;
        let object = if self.config.raw_objects {
            sink.object(&self.values[base..], offset)
        } else {
            let (pairs, repeated) = self.unique_pairs(base, keys);
            if let Some(key) = repeated {
                match self.config.duplicates {
                    Duplicates::Ignore => {}
                    Duplicates::Warn => {
                        let (line, column) = position(self.input, offset);
                        self.warnings.push(Warning { key, line, column });
                    }
                    Duplicates::Raise => {
                        let (line, column) = position(self.input, offset);
                        return Err(Failure::Duplicate {
                            key,
                            fragment: fragment(self.input, offset),
                            line,
                            column,
                        });
                    }
                }
            }
            match pairs {
                Some(pairs) => sink.object(&pairs, offset),
                None => sink.object(&self.values[base..], offset),
            }
        };
        self.values.truncate(base);
        if let Some(&(start, _)) = self.keys.get(keys) {
            self.key_bytes.truncate(start);
        }
        self.keys.truncate(keys);
        Ok(object)
    }

    /// The object's pairs with each repeated key folded into its first
    /// place, and the first key that repeats; `None` for the pairs when none
    /// does, so the common case copies nothing.
    #[allow(clippy::type_complexity)]
    fn unique_pairs(&self, base: usize, keys: usize) -> (Option<Vec<V>>, Option<V>) {
        let flat = &self.values[base..];
        let names: Vec<&[u8]> = self.keys[keys..]
            .iter()
            .map(|&(start, end)| &self.key_bytes[start..end])
            .collect();
        let count = names.len();
        let mut first_repeat = None;
        let mut slots: Vec<usize> = Vec::with_capacity(count);
        let mut out: Vec<V> = Vec::new();
        let mut index: HashMap<&[u8], usize> = HashMap::new();
        for (i, name) in names.iter().enumerate() {
            let seen = if count > 8 {
                index.get(name).copied()
            } else {
                names[..i].iter().position(|earlier| earlier == name)
            };
            match seen {
                Some(earlier) => {
                    if first_repeat.is_none() {
                        first_repeat = Some(flat[2 * i]);
                        out = flat[..2 * i].to_vec();
                        slots = (0..i).collect();
                    }
                    let slot = slots[earlier];
                    out[2 * slot + 1] = flat[2 * i + 1];
                    slots.push(slot);
                }
                None => {
                    if count > 8 {
                        index.insert(name, i);
                    }
                    if first_repeat.is_some() {
                        slots.push(out.len() / 2);
                        out.push(flat[2 * i]);
                        out.push(flat[2 * i + 1]);
                    }
                }
            }
        }
        match first_repeat {
            Some(key) => (Some(out), Some(key)),
            None => (None, None),
        }
    }

    fn scalar<S: Sink<Value = V>>(&mut self, sink: &mut S) -> Step<V, V> {
        let config = self.config;
        match self.peek() {
            b'n' => {
                if self.starts_with(self.cursor, b"null") {
                    self.cursor += 4;
                    return Ok(sink.literal(Literal::Null));
                }
                Err(self.error("unexpected token %s"))
            }
            b't' => {
                if self.starts_with(self.cursor, b"true") {
                    self.cursor += 4;
                    return Ok(sink.literal(Literal::True));
                }
                Err(self.error("unexpected token %s"))
            }
            b'f' => {
                if self.starts_with(self.cursor, b"false") {
                    self.cursor += 5;
                    return Ok(sink.literal(Literal::False));
                }
                Err(self.error("unexpected token %s"))
            }
            b'N' => {
                if config.allow_nan && self.starts_with(self.cursor, b"NaN") {
                    self.cursor += 3;
                    return Ok(sink.literal(Literal::NaN));
                }
                Err(self.error("unexpected token %s"))
            }
            b'I' => {
                if config.allow_nan && self.starts_with(self.cursor, b"Infinity") {
                    self.cursor += 8;
                    return Ok(sink.literal(Literal::Infinity));
                }
                Err(self.error("unexpected token %s"))
            }
            b'-' => {
                if self.rest() >= 9 && self.starts_with(self.cursor + 1, b"Infinity") {
                    if config.allow_nan {
                        self.cursor += 9;
                        return Ok(sink.literal(Literal::MinusInfinity));
                    }
                    return Err(self.error("unexpected token %s"));
                }
                let start = self.cursor;
                self.cursor += 1;
                self.number(sink, true, start)
            }
            b'0'..=b'9' => {
                let start = self.cursor;
                self.number(sink, false, start)
            }
            b'"' => self.string(sink, false),
            0 => Err(self.error("unexpected end of input")),
            _ => Err(self.error("unexpected character: %s")),
        }
    }

    /// A run of digits, accumulated as the C parser accumulates them: into a
    /// `u64` that wraps.
    fn digits(&mut self, accumulator: &mut u64) -> i64 {
        let start = self.cursor;
        while self.peek().is_ascii_digit() {
            *accumulator = accumulator
                .wrapping_mul(10)
                .wrapping_add(u64::from(self.peek() - b'0'));
            self.cursor += 1;
        }
        (self.cursor - start) as i64
    }

    fn number<S: Sink<Value = V>>(
        &mut self,
        sink: &mut S,
        negative: bool,
        start: usize,
    ) -> Step<V, V> {
        let first = self.peek();
        let mut integer = true;
        let mut mantissa = 0u64;
        let mut point = -1i64;
        let mut digits = self.digits(&mut mantissa);
        if (first == b'0' && digits > 1) || (negative && digits == 0) {
            return Err(self.error_at("invalid number: %s", start));
        }
        if self.peek() == b'.' {
            integer = false;
            point = digits;
            self.cursor += 1;
            let fraction = self.digits(&mut mantissa);
            digits += fraction;
            if fraction == 0 {
                return Err(self.error_at("invalid number: %s", start));
            }
        }
        let mut exponent: i32 = 0;
        if self.peek().eq_ignore_ascii_case(&b'e') {
            integer = false;
            self.cursor += 1;
            let mut negative_exponent = false;
            if matches!(self.peek(), b'-' | b'+') {
                negative_exponent = self.peek() == b'-';
                self.cursor += 1;
            }
            let mut magnitude = 0u64;
            if self.digits(&mut magnitude) == 0 {
                return Err(self.error_at("invalid number: %s", start));
            }
            // `(int32_t)abs_exponent`, negated: both wrap.
            let truncated = magnitude as i32;
            exponent = if negative_exponent {
                truncated.wrapping_neg()
            } else {
                truncated
            };
        }
        let text = &self.input[start..self.cursor];
        if integer {
            let small = (digits < 18).then(|| {
                let magnitude = mantissa as i64;
                if negative { -magnitude } else { magnitude }
            });
            return Ok(sink.integer(small, text));
        }
        if point >= 0 {
            exponent = exponent.wrapping_sub((digits - point) as i32);
        }
        let value = if digits > 17 || (digits as i32).wrapping_add(exponent) < -307 {
            decimal(text)
        } else {
            from_parts(mantissa, digits as i32, exponent, negative)
        };
        Ok(sink.float(value, text))
    }

    fn string<S: Sink<Value = V>>(&mut self, sink: &mut S, key: bool) -> Step<V, V> {
        self.cursor += 1;
        let start = self.cursor;
        // The first byte that ends the fast path: a quote, a backslash or a
        // control character.
        if !self.scan() {
            return Err(self.error("unexpected end of input, expected closing \""));
        }
        let input = self.input;
        let unescaped;
        let bytes: &[u8] = if self.peek() == b'"' {
            &input[start..self.cursor]
        } else {
            loop {
                match self.peek() {
                    b'"' => break,
                    b'\\' => self.cursor += 1,
                    _ => {
                        if !self.config.allow_control_characters {
                            return Err(self.error("invalid ASCII control character in string: %s"));
                        }
                    }
                }
                self.cursor += 1;
                if !self.scan() {
                    return Err(self.error("unexpected end of input, expected closing \""));
                }
            }
            unescaped = self.unescape(start, self.cursor)?;
            &unescaped
        };
        self.cursor += 1;
        if key && !self.config.raw_objects {
            let at = self.key_bytes.len();
            self.key_bytes.extend_from_slice(bytes);
            self.keys.push((at, self.key_bytes.len()));
        }
        Ok(sink.string(bytes, key))
    }

    /// Advance to the next quote, backslash or control character; false at
    /// the end of the input (or past it, after a trailing backslash).
    fn scan(&mut self) -> bool {
        while self.cursor < self.input.len() {
            let b = self.input[self.cursor];
            if b == b'"' || b == b'\\' || b < 0x20 {
                return true;
            }
            self.cursor += 1;
        }
        false
    }

    fn unicode(&self, hex: usize) -> Step<u32, V> {
        let mut result = 0u32;
        for i in 0..4 {
            let digit = match self.at(hex + i) {
                b @ b'0'..=b'9' => b - b'0',
                b @ b'a'..=b'f' => b - b'a' + 10,
                b @ b'A'..=b'F' => b - b'A' + 10,
                _ => {
                    return Err(self.error_at(
                        "incomplete unicode character escape sequence at %s",
                        hex - 2,
                    ));
                }
            };
            result = (result << 4) | u32::from(digit);
        }
        Ok(result)
    }

    /// `json_string_unescape` over `start..end`, which the scan has already
    /// checked holds no unescaped quote. `p` is the start of the bytes not
    /// yet copied and `pe` the escape being read, as there.
    fn unescape(&self, start: usize, end: usize) -> Step<Vec<u8>, V> {
        let input = self.input;
        let mut out = Vec::with_capacity(end - start);
        let mut p = start;
        let mut pe = start;
        while pe < end {
            let Some(found) = input[pe..end].iter().position(|&b| b == b'\\') else {
                break;
            };
            pe += found;
            out.extend_from_slice(&input[p..pe]);
            pe += 1;
            match self.at(pe) {
                b'"' | b'/' => p = pe,
                b'\\' => {
                    out.push(b'\\');
                    pe += 1;
                    p = pe;
                }
                c @ (b'n' | b'r' | b't' | b'b' | b'f') => {
                    out.push(match c {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'b' => 0x08,
                        _ => 0x0c,
                    });
                    pe += 1;
                    p = pe;
                }
                b'u' => {
                    if pe + 5 > end {
                        return Err(
                            self.error_at("incomplete unicode character escape sequence at %s", p)
                        );
                    }
                    pe += 1;
                    let mut ch = self.unicode(pe)?;
                    pe += 3;
                    if ch & 0xFC00 == 0xD800 {
                        pe += 1;
                        if pe + 6 > end {
                            return Err(self.error_at("incomplete surrogate pair at %s", p));
                        }
                        if self.at(pe) == b'\\' && self.at(pe + 1) == b'u' {
                            let low = self.unicode(pe + 2)?;
                            if low & 0xFC00 != 0xDC00 {
                                return Err(self.error_at("invalid surrogate pair at %s", p));
                            }
                            ch = ((ch & 0x3F) << 10)
                                | ((((ch >> 6) & 0xF) + 1) << 16)
                                | (low & 0x3FF);
                            pe += 5;
                        } else {
                            return Err(self.error_at("incomplete surrogate pair at %s", p));
                        }
                    }
                    utf8(ch, &mut out);
                    pe += 1;
                    p = pe;
                }
                c => {
                    if c < 0x20 {
                        if !self.config.allow_control_characters {
                            if c == b'\n' {
                                return Err(self.error_at(
                                    "Invalid unescaped newline character (\\n) in string: %s",
                                    pe - 1,
                                ));
                            }
                            return Err(self.error_at(
                                "invalid ASCII control character in string: %s",
                                pe - 1,
                            ));
                        }
                        // Allowed, so kept as it stands, backslash and all:
                        // `p` stays where it was.
                    } else {
                        return Err(self.error_at("invalid escape character in string: %s", pe - 1));
                    }
                }
            }
        }
        if end > p {
            out.extend_from_slice(&input[p..end]);
        }
        Ok(out)
    }

    fn whitespace(&mut self) -> Step<(), V> {
        loop {
            match self.peek() {
                b' ' | b'\n' | b'\t' | b'\r' => self.cursor += 1,
                b'/' => self.comment()?,
                _ => return Ok(()),
            }
        }
    }

    fn comment(&mut self) -> Step<(), V> {
        let start = self.cursor;
        self.cursor += 1;
        match self.peek() {
            b'/' => {
                let rest = &self.input[self.cursor..];
                self.cursor = match rest.iter().position(|&b| b == b'\n') {
                    Some(newline) => self.cursor + newline + 1,
                    None => self.input.len(),
                };
            }
            b'*' => {
                self.cursor += 1;
                loop {
                    let rest = self.input.get(self.cursor..).unwrap_or(&[]);
                    let Some(star) = rest.iter().position(|&b| b == b'*') else {
                        return Err(
                            self.error_at("unterminated comment, expected closing '*/'", start)
                        );
                    };
                    self.cursor += star + 1;
                    if self.peek() == b'/' {
                        self.cursor += 1;
                        break;
                    }
                }
            }
            _ => return Err(self.error_at("unexpected token %s", start)),
        }
        Ok(())
    }
}

/// `convert_UTF32_to_UTF8`: a code point as UTF-8, surrogates included, as
/// the C parser writes them.
fn utf8(ch: u32, out: &mut Vec<u8>) {
    if ch <= 0x7F {
        out.push(ch as u8);
    } else if ch <= 0x07FF {
        out.push(((ch >> 6) | 0xC0) as u8);
        out.push(((ch & 0x3F) | 0x80) as u8);
    } else if ch <= 0xFFFF {
        out.push(((ch >> 12) | 0xE0) as u8);
        out.push((((ch >> 6) & 0x3F) | 0x80) as u8);
        out.push(((ch & 0x3F) | 0x80) as u8);
    } else if ch <= 0x1F_FFFF {
        out.push(((ch >> 18) | 0xF0) as u8);
        out.push((((ch >> 12) & 0x3F) | 0x80) as u8);
        out.push((((ch >> 6) & 0x3F) | 0x80) as u8);
        out.push(((ch & 0x3F) | 0x80) as u8);
    } else {
        out.push(b'?');
    }
}

/// A number's text as `rb_cstr_to_dbl` reads it: correctly rounded, the
/// grammar having already checked it is a JSON number.
fn decimal(text: &[u8]) -> f64 {
    std::str::from_utf8(text)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// `ryu_s2d_from_parts`: `mantissa * 10**exponent`, with `digits` the count
/// of digits the mantissa was written with, leading zeros included. Ryu
/// answers zero and the infinities early from `digits + exponent` alone,
/// which a mantissa written with leading zeros (`0.0001e310`) can tip over;
/// past those checks it is correctly rounded, as Rust's parse is.
fn from_parts(mantissa: u64, digits: i32, exponent: i32, negative: bool) -> f64 {
    let signed = |f: f64| if negative { -f } else { f };
    if mantissa == 0 || digits.wrapping_add(exponent) <= -324 {
        return signed(0.0);
    }
    if digits.wrapping_add(exponent) >= 310 {
        return signed(f64::INFINITY);
    }
    signed(
        format!("{mantissa}e{exponent}")
            .parse::<f64>()
            .unwrap_or(0.0),
    )
}

/// `convert_UTF8_to_JSON` and its `script_safe` and `ascii_only` variants
/// from json 2.18.0's generator: `bytes` as the inside of a JSON string,
/// appended to `out`. Always escaped are `"`, `\\` and the control
/// characters (`\b`, `\f`, `\n`, `\r` and `\t` by name, the rest as `\u00XX`);
/// under `script_safe` also `/`, U+2028 and U+2029; under `ascii_only` every
/// character past ASCII, as `\uXXXX` or a surrogate pair. `bytes` is valid
/// UTF-8, which the Ruby half has checked; a byte that does not start a
/// character is copied as it stands.
pub fn escape(bytes: &[u8], script_safe: bool, ascii_only: bool, out: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let unicode = |code: u32, out: &mut Vec<u8>| {
        out.extend_from_slice(b"\\u");
        for shift in [12, 8, 4, 0] {
            out.push(HEX[((code >> shift) & 0xF) as usize]);
        }
    };
    let mut i = 0;
    let mut copied = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let named: Option<&[u8]> = match b {
            b'"' => Some(b"\\\""),
            b'\\' => Some(b"\\\\"),
            b'/' if script_safe => Some(b"\\/"),
            0x08 => Some(b"\\b"),
            0x0C => Some(b"\\f"),
            b'\n' => Some(b"\\n"),
            b'\r' => Some(b"\\r"),
            b'\t' => Some(b"\\t"),
            _ => None,
        };
        if let Some(name) = named {
            out.extend_from_slice(&bytes[copied..i]);
            out.extend_from_slice(name);
            i += 1;
            copied = i;
            continue;
        }
        if b < 0x20 {
            out.extend_from_slice(&bytes[copied..i]);
            unicode(u32::from(b), out);
            i += 1;
            copied = i;
            continue;
        }
        if b < 0x80 {
            i += 1;
            continue;
        }
        let len = match b {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        };
        let Some(sequence) = bytes.get(i..i + len).filter(|_| len > 1) else {
            i += 1;
            continue;
        };
        if ascii_only {
            let mut code = u32::from(b) & (0x7F >> len);
            for &continuation in &sequence[1..] {
                code = (code << 6) | (u32::from(continuation) & 0x3F);
            }
            out.extend_from_slice(&bytes[copied..i]);
            if code <= 0xFFFF {
                unicode(code, out);
            } else {
                let code = code - 0x10000;
                unicode(0xD800 + (code >> 10), out);
                unicode(0xDC00 + (code & 0x3FF), out);
            }
            i += len;
            copied = i;
            continue;
        }
        if script_safe && sequence[0] == 0xE2 && sequence[1] == 0x80 && (sequence[2] >> 1) == 0x54 {
            out.extend_from_slice(&bytes[copied..i]);
            out.extend_from_slice(if sequence[2] & 1 == 1 {
                b"\\u2029"
            } else {
                b"\\u2028"
            });
            i += len;
            copied = i;
            continue;
        }
        i += len;
    }
    out.extend_from_slice(&bytes[copied..]);
}

/// `fpconv_dtoa` from json 2.18.0's `ext/json/ext/vendor/fpconv.c`: a finite
/// `Float` as the C generator writes it. The digits are Grisu2's, which are
/// not always the shortest `Float#to_s` would print (about one double in
/// seven hundred differs in its last digit or two), and the layout is the
/// gem's own: `1e+20` where Ruby has `1.0e+20`, `0.00000015` where Ruby has
/// `1.5e-07`. The arithmetic is 64-bit and wraps where the C code's does.
pub fn dtoa(d: f64) -> String {
    let mut out = String::with_capacity(28);
    let bits = d.to_bits();
    let negative = bits & SIGN_MASK != 0;
    if negative {
        out.push('-');
    }
    if d == 0.0 {
        out.push_str("0.0");
        return out;
    }
    if bits & EXP_MASK == EXP_MASK {
        out.push_str(if bits & FRAC_MASK != 0 { "nan" } else { "inf" });
        return out;
    }
    let mut digits = [0u8; 18];
    let mut k = 0;
    let count = grisu2(d, &mut digits, &mut k);
    emit_digits(&digits[..count], k, negative, &mut out);
    out
}

const FRAC_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;
const EXP_MASK: u64 = 0x7FF0_0000_0000_0000;
const HIDDEN_BIT: u64 = 0x0010_0000_0000_0000;
const SIGN_MASK: u64 = 0x8000_0000_0000_0000;
const EXP_BIAS: i32 = 1023 + 52;

#[derive(Clone, Copy)]
struct Fp {
    frac: u64,
    exp: i32,
}

/// `powers_ten`: 10**k for k from -348 in steps of 8, as normalised 64-bit
/// fractions and binary exponents.
const POWERS_TEN: [(u64, i32); 87] = [
    (18054884314459144840, -1220),
    (13451937075301367670, -1193),
    (10022474136428063862, -1166),
    (14934650266808366570, -1140),
    (11127181549972568877, -1113),
    (16580792590934885855, -1087),
    (12353653155963782858, -1060),
    (18408377700990114895, -1034),
    (13715310171984221708, -1007),
    (10218702384817765436, -980),
    (15227053142812498563, -954),
    (11345038669416679861, -927),
    (16905424996341287883, -901),
    (12595523146049147757, -874),
    (9384396036005875287, -847),
    (13983839803942852151, -821),
    (10418772551374772303, -794),
    (15525180923007089351, -768),
    (11567161174868858868, -741),
    (17236413322193710309, -715),
    (12842128665889583758, -688),
    (9568131466127621947, -661),
    (14257626930069360058, -635),
    (10622759856335341974, -608),
    (15829145694278690180, -582),
    (11793632577567316726, -555),
    (17573882009934360870, -529),
    (13093562431584567480, -502),
    (9755464219737475723, -475),
    (14536774485912137811, -449),
    (10830740992659433045, -422),
    (16139061738043178685, -396),
    (12024538023802026127, -369),
    (17917957937422433684, -343),
    (13349918974505688015, -316),
    (9946464728195732843, -289),
    (14821387422376473014, -263),
    (11042794154864902060, -236),
    (16455045573212060422, -210),
    (12259964326927110867, -183),
    (18268770466636286478, -157),
    (13611294676837538539, -130),
    (10141204801825835212, -103),
    (15111572745182864684, -77),
    (11258999068426240000, -50),
    (16777216000000000000, -24),
    (12500000000000000000, 3),
    (9313225746154785156, 30),
    (13877787807814456755, 56),
    (10339757656912845936, 83),
    (15407439555097886824, 109),
    (11479437019748901445, 136),
    (17105694144590052135, 162),
    (12744735289059618216, 189),
    (9495567745759798747, 216),
    (14149498560666738074, 242),
    (10542197943230523224, 269),
    (15709099088952724970, 295),
    (11704190886730495818, 322),
    (17440603504673385349, 348),
    (12994262207056124023, 375),
    (9681479787123295682, 402),
    (14426529090290212157, 428),
    (10748601772107342003, 455),
    (16016664761464807395, 481),
    (11933345169920330789, 508),
    (17782069995880619868, 534),
    (13248674568444952270, 561),
    (9871031767461413346, 588),
    (14708983551653345445, 614),
    (10959046745042015199, 641),
    (16330252207878254650, 667),
    (12166986024289022870, 694),
    (18130221999122236476, 720),
    (13508068024458167312, 747),
    (10064294952495520794, 774),
    (14996968138956309548, 800),
    (11173611982879273257, 827),
    (16649979327439178909, 853),
    (12405201291620119593, 880),
    (9242595204427927429, 907),
    (13772540099066387757, 933),
    (10261342003245940623, 960),
    (15290591125556738113, 986),
    (11392378155556871081, 1013),
    (16975966327722178521, 1039),
    (12648080533535911531, 1066),
];

const TENS: [u64; 20] = [
    10_000_000_000_000_000_000,
    1_000_000_000_000_000_000,
    100_000_000_000_000_000,
    10_000_000_000_000_000,
    1_000_000_000_000_000,
    100_000_000_000_000,
    10_000_000_000_000,
    1_000_000_000_000,
    100_000_000_000,
    10_000_000_000,
    1_000_000_000,
    100_000_000,
    10_000_000,
    1_000_000,
    100_000,
    10_000,
    1_000,
    100,
    10,
    1,
];

fn find_cached_pow10(exp: i32, k: &mut i32) -> Fp {
    const ONE_LOG_TEN: f64 = 0.301_029_995_663_981_14;
    let approx = (f64::from(-(exp + 87)) * ONE_LOG_TEN) as i32;
    let mut idx = ((approx + 348) / 8).clamp(0, 86) as usize;
    loop {
        let current = exp + POWERS_TEN[idx].1 + 64;
        if current < -60 && idx < 86 {
            idx += 1;
            continue;
        }
        if current > -32 && idx > 0 {
            idx -= 1;
            continue;
        }
        *k = -348 + idx as i32 * 8;
        let (frac, exp) = POWERS_TEN[idx];
        return Fp { frac, exp };
    }
}

fn build_fp(d: f64) -> Fp {
    let bits = d.to_bits();
    let mut fp = Fp {
        frac: bits & FRAC_MASK,
        exp: ((bits & EXP_MASK) >> 52) as i32,
    };
    if fp.exp != 0 {
        fp.frac += HIDDEN_BIT;
        fp.exp -= EXP_BIAS;
    } else {
        fp.exp = -EXP_BIAS + 1;
    }
    fp
}

fn normalize(fp: &mut Fp) {
    while fp.frac & HIDDEN_BIT == 0 {
        fp.frac <<= 1;
        fp.exp -= 1;
    }
    let shift = 64 - 52 - 1;
    fp.frac <<= shift;
    fp.exp -= shift;
}

fn normalized_boundaries(fp: &Fp) -> (Fp, Fp) {
    let mut upper = Fp {
        frac: (fp.frac << 1) + 1,
        exp: fp.exp - 1,
    };
    while upper.frac & (HIDDEN_BIT << 1) == 0 {
        upper.frac <<= 1;
        upper.exp -= 1;
    }
    let u_shift = 64 - 52 - 2;
    upper.frac <<= u_shift;
    upper.exp -= u_shift;
    let l_shift = if fp.frac == HIDDEN_BIT { 2 } else { 1 };
    let mut lower = Fp {
        frac: (fp.frac << l_shift) - 1,
        exp: fp.exp - l_shift,
    };
    lower.frac <<= lower.exp - upper.exp;
    lower.exp = upper.exp;
    (lower, upper)
}

fn multiply(a: &Fp, b: &Fp) -> Fp {
    const LO: u64 = 0xFFFF_FFFF;
    let ah_bl = (a.frac >> 32) * (b.frac & LO);
    let al_bh = (a.frac & LO) * (b.frac >> 32);
    let al_bl = (a.frac & LO) * (b.frac & LO);
    let ah_bh = (a.frac >> 32) * (b.frac >> 32);
    let mut tmp = (ah_bl & LO) + (al_bh & LO) + (al_bl >> 32);
    tmp += 1 << 31;
    Fp {
        frac: ah_bh + (ah_bl >> 32) + (al_bh >> 32) + (tmp >> 32),
        exp: a.exp + b.exp + 64,
    }
}

fn round_digit(digits: &mut [u8], count: usize, delta: u64, mut rem: u64, kappa: u64, frac: u64) {
    while rem < frac
        && delta.wrapping_sub(rem) >= kappa
        && (rem.wrapping_add(kappa) < frac
            || frac - rem > rem.wrapping_add(kappa).wrapping_sub(frac))
    {
        digits[count - 1] -= 1;
        rem = rem.wrapping_add(kappa);
    }
}

fn generate_digits(fp: &Fp, upper: &Fp, lower: &Fp, digits: &mut [u8; 18], k: &mut i32) -> usize {
    let wfrac = upper.frac.wrapping_sub(fp.frac);
    let mut delta = upper.frac.wrapping_sub(lower.frac);
    let shift = (-upper.exp) as u32;
    let one = 1u64 << shift;
    let mut part1 = upper.frac >> shift;
    let mut part2 = upper.frac & (one - 1);
    let mut idx = 0usize;
    let mut kappa = 10;
    for &div in &TENS[10..] {
        if kappa <= 0 {
            break;
        }
        let digit = part1 / div;
        if digit != 0 || idx != 0 {
            if idx < digits.len() {
                digits[idx] = digit as u8 + b'0';
            }
            idx += 1;
        }
        part1 -= digit * div;
        kappa -= 1;
        let tmp = (part1 << shift).wrapping_add(part2);
        if tmp <= delta {
            *k += kappa;
            let count = idx.min(digits.len());
            round_digit(digits, count, delta, tmp, div << shift, wfrac);
            return count;
        }
    }
    let mut unit = 18usize;
    loop {
        part2 = part2.wrapping_mul(10);
        delta = delta.wrapping_mul(10);
        kappa -= 1;
        let digit = part2 >> shift;
        if digit != 0 || idx != 0 {
            if idx < digits.len() {
                digits[idx] = digit as u8 + b'0';
            }
            idx += 1;
        }
        part2 &= one - 1;
        if part2 < delta {
            *k += kappa;
            let count = idx.min(digits.len());
            round_digit(
                digits,
                count,
                delta,
                part2,
                one,
                wfrac.wrapping_mul(TENS[unit]),
            );
            return count;
        }
        unit = unit.saturating_sub(1);
    }
}

fn grisu2(d: f64, digits: &mut [u8; 18], k: &mut i32) -> usize {
    let mut w = build_fp(d);
    let (lower, upper) = normalized_boundaries(&w);
    normalize(&mut w);
    let mut cached = 0;
    let cp = find_cached_pow10(upper.exp, &mut cached);
    let w = multiply(&w, &cp);
    let mut upper = multiply(&upper, &cp);
    let mut lower = multiply(&lower, &cp);
    lower.frac += 1;
    upper.frac -= 1;
    *k = -cached;
    generate_digits(&w, &upper, &lower, digits, k)
}

fn emit_digits(digits: &[u8], k: i32, negative: bool, out: &mut String) {
    let ndigits = digits.len() as i32;
    let exp = (k + ndigits - 1).abs();
    let text = |d: &[u8]| String::from_utf8_lossy(d).into_owned();
    if k >= 0 && exp < 15 {
        out.push_str(&text(digits));
        out.extend(std::iter::repeat_n('0', k as usize));
        out.push_str(".0");
        return;
    }
    if k < 0 && (k > -7 || exp < 10) {
        let offset = ndigits - k.abs();
        if offset <= 0 {
            out.push_str("0.");
            out.extend(std::iter::repeat_n('0', (-offset) as usize));
            out.push_str(&text(digits));
        } else {
            let offset = offset as usize;
            out.push_str(&text(&digits[..offset]));
            out.push('.');
            out.push_str(&text(&digits[offset..]));
        }
        return;
    }
    let shown = ndigits.min(18 - i32::from(negative)) as usize;
    out.push(char::from(digits[0]));
    if shown > 1 {
        out.push('.');
        out.push_str(&text(&digits[1..shown]));
    }
    out.push('e');
    out.push(if k + ndigits - 1 < 0 { '-' } else { '+' });
    out.push_str(&exp.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How big the big inputs are: a million, or a thousand under miri, where
    /// what is being checked is the memory safety of the same code paths.
    const SCALE: usize = if cfg!(miri) { 1_000 } else { 1_000_000 };

    /// A tree to build in place of Ruby objects, printed the way `p` would
    /// print what the Ruby half builds, near enough to compare by eye.
    #[derive(Debug, Clone, PartialEq)]
    enum Tree {
        Null,
        Bool(bool),
        Int(String),
        Float(f64),
        Str(Vec<u8>),
        Array(Vec<Tree>),
        Object(Vec<(Tree, Tree)>),
    }

    #[derive(Default)]
    struct Trees {
        nodes: Vec<Tree>,
    }

    impl Sink for Trees {
        type Value = usize;
        fn literal(&mut self, which: Literal) -> usize {
            self.push(match which {
                Literal::Null => Tree::Null,
                Literal::True => Tree::Bool(true),
                Literal::False => Tree::Bool(false),
                Literal::NaN => Tree::Float(f64::NAN),
                Literal::Infinity => Tree::Float(f64::INFINITY),
                Literal::MinusInfinity => Tree::Float(f64::NEG_INFINITY),
            })
        }
        fn integer(&mut self, small: Option<i64>, text: &[u8]) -> usize {
            let shown = match small {
                Some(n) => n.to_string(),
                None => String::from_utf8_lossy(text).into_owned(),
            };
            self.push(Tree::Int(shown))
        }
        fn float(&mut self, value: f64, _text: &[u8]) -> usize {
            self.push(Tree::Float(value))
        }
        fn string(&mut self, bytes: &[u8], _key: bool) -> usize {
            self.push(Tree::Str(bytes.to_vec()))
        }
        fn array(&mut self, items: &[usize]) -> usize {
            let items = items.iter().map(|&i| self.nodes[i].clone()).collect();
            self.push(Tree::Array(items))
        }
        fn object(&mut self, pairs: &[usize], _offset: usize) -> usize {
            let pairs = pairs
                .chunks(2)
                .map(|kv| (self.nodes[kv[0]].clone(), self.nodes[kv[1]].clone()))
                .collect();
            self.push(Tree::Object(pairs))
        }
    }

    impl Trees {
        fn push(&mut self, tree: Tree) -> usize {
            self.nodes.push(tree);
            self.nodes.len() - 1
        }
    }

    fn run(input: &[u8], config: &Config) -> Result<Tree, String> {
        let mut trees = Trees::default();
        let outcome = parse(input, config, &mut trees);
        match outcome.result {
            Ok(i) => Ok(trees.nodes[i].clone()),
            Err(Failure::Syntax { message, .. }) => {
                Err(String::from_utf8_lossy(&message).into_owned())
            }
            Err(Failure::Nesting { depth }) => Err(format!("nesting of {depth} is too deep")),
            Err(Failure::Duplicate { line, column, .. }) => {
                Err(format!("duplicate at line {line} column {column}"))
            }
        }
    }

    fn ok(input: &str) -> Tree {
        run(input.as_bytes(), &Config::default()).unwrap_or_else(|e| panic!("{input:?}: {e}"))
    }

    fn err(input: &[u8]) -> String {
        match run(input, &Config::default()) {
            Ok(tree) => panic!("{input:?} parsed as {tree:?}"),
            Err(message) => message,
        }
    }

    // The literals are the Ruby ones the expected strings were printed from.
    #[allow(clippy::excessive_precision)]
    #[test]
    fn floats_print_as_fpconv_prints_them() {
        // Each from ruby 4.0.9's `JSON.generate`.
        let cases: &[(f64, &str)] = &[
            (1.0, "1.0"),
            (-1.0, "-1.0"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (12.2, "12.2"),
            (100.0, "100.0"),
            (1e20, "1e+20"),
            (1e16, "1e+16"),
            (1.5e-7, "0.00000015"),
            (123456789.123456789, "123456789.12345679"),
            (100000000000000.0, "100000000000000.0"),
            (1e15, "1e+15"),
            (11111111111111111.1, "1.1111111111111112e+16"),
            (-2.2471348024634545e-08, "-0.000000022471348024634545"),
            (-2.2471348024634545e-10, "-2.2471348024634546e-10"),
            (4.3336577655099307e-85, "4.3336577655099306e-85"),
            (-3.394068756905255e+16, "-3.3940687569052552e+16"),
            (149193113271222.88, "149193113271222.87"),
            (8561358174.655478, "8561358174.6554775"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (2.2250738585072014e-308, "2.2250738585072014e-308"),
        ];
        for &(f, expected) in cases {
            assert_eq!(dtoa(f), expected, "{f:e}");
        }
    }

    #[test]
    fn strings_escape_as_the_c_generator_escapes_them() {
        // Each from ruby 4.0.9's `JSON.generate`, quotes taken off.
        let run = |text: &str, script_safe: bool, ascii_only: bool| {
            let mut out = Vec::new();
            escape(text.as_bytes(), script_safe, ascii_only, &mut out);
            String::from_utf8(out).expect("escaping keeps UTF-8")
        };
        let all = "é\u{2028}\u{2029}/\u{1}\u{1f}\"\\\u{8}\u{c}\n\r\t😀";
        assert_eq!(
            run(all, false, false),
            "é\u{2028}\u{2029}/\\u0001\\u001f\\\"\\\\\\b\\f\\n\\r\\t😀"
        );
        assert_eq!(
            run("é\u{2028}/</script>", true, false),
            "é\\u2028\\/<\\/script>"
        );
        assert_eq!(
            run("é😀\u{2028}/\u{1}", false, true),
            "\\u00e9\\ud83d\\ude00\\u2028/\\u0001"
        );
        assert_eq!(
            run("é😀\u{2028}/", true, true),
            "\\u00e9\\ud83d\\ude00\\u2028\\/"
        );
        assert_eq!(run("plain", true, true), "plain");
        assert_eq!(run("\u{2027}\u{202a}", true, false), "\u{2027}\u{202a}");
    }

    #[test]
    fn values_parse() {
        assert_eq!(ok(" null "), Tree::Null);
        assert_eq!(
            ok("[1, -2, 3.5, true, false, \"x\"]"),
            Tree::Array(vec![
                Tree::Int("1".into()),
                Tree::Int("-2".into()),
                Tree::Float(3.5),
                Tree::Bool(true),
                Tree::Bool(false),
                Tree::Str(b"x".to_vec()),
            ])
        );
        assert_eq!(
            ok("{\"a\": {\"b\": []}, \"c\": {}}"),
            Tree::Object(vec![
                (
                    Tree::Str(b"a".to_vec()),
                    Tree::Object(vec![(Tree::Str(b"b".to_vec()), Tree::Array(vec![]))])
                ),
                (Tree::Str(b"c".to_vec()), Tree::Object(vec![])),
            ])
        );
        assert_eq!(
            ok("123456789012345678901234567890"),
            Tree::Int("123456789012345678901234567890".into())
        );
        assert_eq!(
            ok(r#""\u00e9\ud83d\ude00\/\b\f\n\r\t\"\\""#),
            Tree::Str("é😀/\u{8}\u{c}\n\r\t\"\\".as_bytes().to_vec())
        );
    }

    #[test]
    fn errors_read_as_the_c_parser_writes_them() {
        // Each from ruby 4.0.9's json 2.18.0.
        let cases: &[(&[u8], &str)] = &[
            (b"", "unexpected end of input at line 1 column 1"),
            (b"\n", "unexpected end of input at line 1 column 1"),
            (b"[1,]", "unexpected character: ']' at line 1 column 4"),
            (
                b"[1 2]",
                "expected ',' or ']' after array value at line 1 column 4",
            ),
            (
                b"{\"a\" 1}",
                "expected ':' after object key at line 1 column 6",
            ),
            (
                b"{\"a\":1,}",
                "expected object key, got: '}' at line 1 column 8",
            ),
            (
                b"{1:2}",
                "expected object key, got '1:2}' at line 1 column 2",
            ),
            (
                b"[\"a\tb\"]",
                "invalid ASCII control character in string: \tb\"] at line 1 column 4",
            ),
            (
                b"[\"a\nb\"]",
                "invalid ASCII control character in string: \nb\"] at line 1 column 0",
            ),
            (
                b"[\"ab\\u12\"]",
                "incomplete unicode character escape sequence at 'ab\\u12\"]' at line 1 column 3",
            ),
            (
                b"[\"xy\\u0041\\u12\"]",
                "incomplete unicode character escape sequence at '\\u12\"]' at line 1 column 11",
            ),
            (
                b"[\"\\ud800\\u0041\"]",
                "invalid surrogate pair at '\\ud800\\u0041\"]' at line 1 column 3",
            ),
            (
                b"[\"\\",
                "unexpected end of input, expected closing \" at line 1 column 5",
            ),
            (
                b"[\"\\t\\\t\"]",
                "invalid ASCII control character in string: '\\' at line 1 column 5",
            ),
            (
                b"[\"\\\n\"]",
                "Invalid unescaped newline character (\\n) in string: '\\' at line 1 column 3",
            ),
            (b"[01]", "invalid number: '01]' at line 1 column 2"),
            (b"[-Inf]", "invalid number: '-Inf]' at line 1 column 2"),
            (
                b"[-Infinity]",
                "unexpected token '-Infinity]' at line 1 column 2",
            ),
            (b"[\x00]", "unexpected end of input at line 1 column 2"),
            (
                b"[1] x",
                "unexpected token at end of stream 'x' at line 1 column 5",
            ),
            (
                b"/*/ [1]",
                "unterminated comment, expected closing '*/' at line 1 column 1",
            ),
            (
                b"[1,\r\n2,\n x]",
                "unexpected character: 'x]' at line 2 column 2",
            ),
            (
                "[xxéééééééééééééééé]".as_bytes(),
                "unexpected character: 'xxéééééééééééééé' at line 1 column 2",
            ),
        ];
        for &(input, expected) in cases {
            assert_eq!(err(input), expected, "{input:?}");
        }
    }

    #[test]
    fn floats_round_as_the_c_parser_rounds_them() {
        let f = |s: &str| match ok(s) {
            Tree::Float(f) => f,
            other => panic!("{s}: {other:?}"),
        };
        assert_eq!(f("1e400"), f64::INFINITY);
        assert_eq!(f("1e-400"), 0.0);
        assert_eq!(f("1e4294967297"), 10.0);
        assert_eq!(f("1e2147483648"), f64::INFINITY);
        assert_eq!(f("0.00001e313"), f64::INFINITY);
        assert!(f("-0.0").is_sign_negative());
        assert_eq!(
            f(&format!("1.{}", "1".repeat(SCALE / 10))),
            1.1111111111111112
        );
    }

    #[test]
    fn nesting_is_counted_and_unbounded_depth_does_not_recurse() {
        let config = Config {
            max_nesting: 3,
            ..Config::default()
        };
        assert_eq!(
            run(b"[[[[1]]]]", &config).unwrap_err(),
            "nesting of 4 is too deep"
        );
        let config = Config {
            max_nesting: 1,
            ..Config::default()
        };
        assert!(run(b"[[]]", &config).is_ok());
        let unchecked = Config {
            max_nesting: 0,
            ..Config::default()
        };
        let depth = SCALE;
        let deep = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let mut trees = Counting(0);
        assert!(
            parse(deep.as_bytes(), &unchecked, &mut trees)
                .result
                .is_ok()
        );
        assert_eq!(trees.0, depth);
    }

    /// Counts what it makes, for inputs too big to build a tree of.
    struct Counting(usize);

    impl Sink for Counting {
        type Value = ();
        fn literal(&mut self, _: Literal) {
            self.0 += 1;
        }
        fn integer(&mut self, _: Option<i64>, _: &[u8]) {
            self.0 += 1;
        }
        fn float(&mut self, _: f64, _: &[u8]) {
            self.0 += 1;
        }
        fn string(&mut self, _: &[u8], _: bool) {
            self.0 += 1;
        }
        fn array(&mut self, _: &[()]) {
            self.0 += 1;
        }
        fn object(&mut self, _: &[()], _: usize) {
            self.0 += 1;
        }
    }

    #[test]
    fn repeated_keys_keep_their_first_place_and_last_value() {
        assert_eq!(
            ok(r#"{"b":1,"a":1,"b":2}"#),
            Tree::Object(vec![
                (Tree::Str(b"b".to_vec()), Tree::Int("2".into())),
                (Tree::Str(b"a".to_vec()), Tree::Int("1".into())),
            ])
        );
        let many: String = (0..20)
            .map(|i| format!("\"k{}\":{i}", i % 10))
            .collect::<Vec<_>>()
            .join(",");
        match ok(&format!("{{{many}}}")) {
            Tree::Object(pairs) => {
                assert_eq!(pairs.len(), 10);
                assert_eq!(pairs[3].1, Tree::Int("13".into()));
            }
            other => panic!("{other:?}"),
        }
        let mut trees = Trees::default();
        let outcome = parse(b"{\n\"a\":1,\"a\":2}", &Config::default(), &mut trees);
        assert_eq!(outcome.warnings.len(), 1);
        assert_eq!(
            (outcome.warnings[0].line, outcome.warnings[0].column),
            (1, 1)
        );
        let raising = Config {
            duplicates: Duplicates::Raise,
            ..Config::default()
        };
        assert_eq!(
            run(b"[{\"a\":1,\"a\":2}]", &raising).unwrap_err(),
            "duplicate at line 1 column 2"
        );
    }

    /// A short, bounded fuzz (20,000 inputs, each under three configs):
    /// random bytes over JSON's alphabet and random cuts of valid documents. The parser must answer every one without
    /// panicking. Deterministic, so a failure reproduces.
    #[test]
    fn arbitrary_input_never_panics() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet: &[u8] =
            b"[]{}:,\"\\/*-+.0123456789eEnulltruefalsNaInfity \n\t\rabu\x00\x01\xc3\xa9\xff";
        let seeds: &[&[u8]] = &[
            br#"{"a":[1,2.5e3,-0,"x\u00e9\ud83d\ude00",{"b":null}],"c":true} "#,
            b"[NaN, Infinity, -Infinity, 1e999, /* c */ 0.1] // tail",
            b"{\"k\":\"\\\\\\\"\\/\\b\\f\\n\\r\\t\"}",
        ];
        let configs = [
            Config::default(),
            Config {
                max_nesting: 0,
                allow_nan: true,
                allow_trailing_comma: true,
                allow_control_characters: true,
                duplicates: Duplicates::Raise,
                raw_objects: false,
            },
            Config {
                raw_objects: true,
                ..Config::default()
            },
        ];
        for round in 0..SCALE / 50 {
            let input: Vec<u8> = if round % 2 == 0 {
                let len = (next() % 64) as usize;
                (0..len)
                    .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                    .collect()
            } else {
                let seed = seeds[(next() % seeds.len() as u64) as usize];
                let mut bytes = seed.to_vec();
                bytes.truncate((next() % (seed.len() as u64 + 1)) as usize);
                if !bytes.is_empty() && next() % 3 == 0 {
                    let at = (next() % bytes.len() as u64) as usize;
                    bytes[at] = alphabet[(next() % alphabet.len() as u64) as usize];
                }
                bytes
            };
            for config in &configs {
                let mut sink = Counting(0);
                let _ = parse(&input, config, &mut sink);
            }
        }
    }
}
