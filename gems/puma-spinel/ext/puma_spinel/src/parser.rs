//! Puma's HTTP request parser: the state machine Ragel 6.10 makes of
//! upstream's `ext/puma_http11/http11_parser.rl` (and the
//! `http11_parser_common.rl` it includes), as `http11_parser.c` runs it,
//! with the length checks `puma_http11.c`'s callbacks make.
//!
//! The machine is the one in the C. Upstream ships it as `-G2` code, gotos
//! a state each; [`tables`] is the same machine as Ragel writes it with
//! `-T0`, tables and a loop over them, from the same `.rl` files
//! (`tables.sh` writes it again, and checks that Ragel 6.10's `-G2` output
//! from them is the `http11_parser.c` upstream ships). [`Parser::execute`]
//! is that loop, and the actions are the `.rl` file's, one for one.
//!
//! What the C callbacks do to Ruby objects — set a key in the request Hash,
//! join a duplicate header, keep the body — is Ruby, in
//! `lib/puma/puma_http11.rb`, replaying the [`Event`]s an `execute` answers
//! in the order the C made its calls. That includes the one write the C
//! makes to the String it parses: `snake_upcase_field` rewrites a header
//! name's bytes in place (`HOST`, `X_FORWARDED_FOR`), which an [`Event::Upcase`]
//! carries back so that a later `execute` on the same String reads them as
//! the C's would.

mod tables;

use std::collections::HashMap;

use tables::{
    ACTIONS, INDEX_OFFSETS, INDICIES, KEY_OFFSETS, RANGE_LENGTHS, SINGLE_LENGTHS, TRANS_ACTIONS,
    TRANS_KEYS, TRANS_TARGS,
};

/// `puma_parser_start`.
const START: i32 = 1;
/// `puma_parser_first_final`.
const FIRST_FINAL: i32 = 46;
/// `puma_parser_error`.
const ERROR: i32 = 0;

/// `DEF_MAX_LENGTH`'s limits, as `puma_http11.c` defines them with no
/// `PUMA_*_MAX_LENGTH` given at compile time, with the names its messages
/// print.
pub const FIELD_NAME: (&str, usize) = ("FIELD_NAME", 256);
pub const FIELD_VALUE: (&str, usize) = ("FIELD_VALUE", 80 * 1024);
pub const REQUEST_URI: (&str, usize) = ("REQUEST_URI", 1024 * 12);
pub const FRAGMENT: (&str, usize) = ("FRAGMENT", 1024);
pub const REQUEST_PATH: (&str, usize) = ("REQUEST_PATH", 8192);
pub const QUERY_STRING: (&str, usize) = ("QUERY_STRING", 1024 * 10);

/// What one `execute` saw, in the order the C made the calls.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// `request_method`: `REQUEST_METHOD`.
    Method(Vec<u8>),
    /// `request_uri`: `REQUEST_URI`.
    Uri(Vec<u8>),
    /// `fragment`: `FRAGMENT`.
    Fragment(Vec<u8>),
    /// `request_path`: `REQUEST_PATH`.
    Path(Vec<u8>),
    /// `query_string`: `QUERY_STRING`.
    Query(Vec<u8>),
    /// `server_protocol`: `SERVER_PROTOCOL`.
    Protocol(Vec<u8>),
    /// `http_field`: a header's name, upcased, and its value with the
    /// spaces and tabs around it left out; the values of a name that came
    /// again, joined.
    Field(Vec<u8>, Vec<u8>),
    /// `header_done`: the bytes after the header, the body's start.
    Body(Vec<u8>),
    /// `snake_upcase_char` ran on bytes from this offset, which are now
    /// these: those it rewrote and any between them.
    Upcase(usize, Vec<u8>),
    /// `VALIDATE_MAX_LENGTH` raised: the element's name, its limit and the
    /// length it was. The C raised out of `execute` here, so nothing after
    /// it happened, and the state and count stayed as they were before the
    /// call.
    TooLong(&'static str, usize, usize),
}

/// `puma_parser`'s state, but for the Ruby objects.
#[derive(Debug, Default)]
pub struct Parser {
    cs: i32,
    body_start: usize,
    nread: usize,
    mark: usize,
    field_start: usize,
    field_len: usize,
    query_start: usize,
}

impl Parser {
    /// A parser as `HttpParser_alloc` leaves it.
    pub fn new() -> Parser {
        let mut parser = Parser::default();
        parser.init();
        parser
    }

    /// `puma_parser_init`. `query_start` is left as it was, as the C leaves
    /// it.
    pub fn init(&mut self) {
        self.cs = START;
        self.body_start = 0;
        self.mark = 0;
        self.nread = 0;
        self.field_len = 0;
        self.field_start = 0;
    }

    pub fn nread(&self) -> usize {
        self.nread
    }

    /// `puma_parser_has_error`.
    pub fn has_error(&self) -> bool {
        self.cs == ERROR
    }

    /// `puma_parser_is_finished`.
    pub fn is_finished(&self) -> bool {
        self.cs >= FIRST_FINAL
    }

    /// `puma_parser_execute` over `buffer` from `off`, with
    /// `puma_http11.c`'s callbacks: what they did, as [`Event`]s. A buffer
    /// is the String's bytes; the caller writes [`Event::Upcase`]'s back.
    ///
    /// Headers come one [`Event::Field`] a name, in the order each name
    /// first came, its values joined by `", "` as `http_field` joins a
    /// duplicate to the first: a request of eighty thousand header lines is
    /// a few events, not eighty thousand. The rewritten bytes come as one
    /// [`Event::Upcase`] spanning them, last, or before an
    /// [`Event::TooLong`].
    ///
    /// # Panics
    ///
    /// If `off` is past the buffer's end, which `HttpParser_execute` raises
    /// for before calling it.
    pub fn execute(&mut self, buffer: &mut [u8], off: usize) -> Vec<Event> {
        assert!(off <= buffer.len(), "offset past end of buffer");
        let mut out = Out::default();
        let pe = buffer.len();
        let mut p = off;
        let mut cs = self.cs;

        // Ragel's `-T0` driver for `%% write exec;`.
        'out: {
            if p == pe || cs == ERROR {
                break 'out;
            }
            loop {
                let state = cs as usize;
                let byte = buffer[p];
                let mut keys = usize::from(KEY_OFFSETS[state]);
                let mut trans = usize::from(INDEX_OFFSETS[state]);
                let mut matched = false;

                let klen = usize::from(SINGLE_LENGTHS[state]);
                if klen > 0 {
                    let singles = &TRANS_KEYS[keys..keys + klen];
                    if let Ok(at) = singles.binary_search(&byte) {
                        trans += at;
                        matched = true;
                    } else {
                        keys += klen;
                        trans += klen;
                    }
                }
                if !matched {
                    let klen = usize::from(RANGE_LENGTHS[state]);
                    if klen > 0 {
                        let ranges = &TRANS_KEYS[keys..keys + 2 * klen];
                        let found = ranges
                            .chunks_exact(2)
                            .position(|range| range[0] <= byte && byte <= range[1]);
                        match found {
                            Some(at) => trans += at,
                            None => trans += klen,
                        }
                    }
                }

                let trans = usize::from(INDICIES[trans]);
                cs = i32::from(TRANS_TARGS[trans]);

                let acts = usize::from(TRANS_ACTIONS[trans]);
                if acts != 0 {
                    let count = usize::from(ACTIONS[acts]);
                    for &action in &ACTIONS[acts + 1..=acts + count] {
                        match action {
                            // mark, start_value
                            0 | 4 => self.mark = p,
                            // start_field
                            1 => self.field_start = p,
                            // snake_upcase_field
                            2 => {
                                snake_upcase_char(&mut buffer[p]);
                                out.upcased = Some(match out.upcased {
                                    Some((from, _)) => (from, p + 1),
                                    None => (p, p + 1),
                                });
                            }
                            // write_field
                            3 => self.field_len = p - self.field_start,
                            // write_value
                            5 => {
                                let name =
                                    &buffer[self.field_start..self.field_start + self.field_len];
                                let value = &buffer[self.mark..p];
                                if let Some(long) = too_long(name.len(), FIELD_NAME)
                                    .or_else(|| too_long(value.len(), FIELD_VALUE))
                                {
                                    return out.raised(long, buffer);
                                }
                                out.field(name, trim_ows(value));
                            }
                            // request_method
                            6 => out
                                .events
                                .push(Event::Method(buffer[self.mark..p].to_vec())),
                            // request_uri, fragment, request_path, query_string
                            7 | 8 | 10 | 12 => {
                                let (from, limit, event): (usize, _, fn(Vec<u8>) -> Event) =
                                    match action {
                                        7 => (self.mark, REQUEST_URI, Event::Uri),
                                        8 => (self.mark, FRAGMENT, Event::Fragment),
                                        10 => (self.query_start, QUERY_STRING, Event::Query),
                                        _ => (self.mark, REQUEST_PATH, Event::Path),
                                    };
                                let bytes = &buffer[from..p];
                                if let Some(long) = too_long(bytes.len(), limit) {
                                    return out.raised(long, buffer);
                                }
                                out.events.push(event(bytes.to_vec()));
                            }
                            // start_query
                            9 => self.query_start = p,
                            // server_protocol
                            11 => out
                                .events
                                .push(Event::Protocol(buffer[self.mark..p].to_vec())),
                            // done
                            13 => {
                                self.body_start = p + 1;
                                out.events.push(Event::Body(buffer[p + 1..pe].to_vec()));
                                p += 1;
                                break 'out;
                            }
                            _ => unreachable!("Ragel wrote no action {action}"),
                        }
                    }
                }

                if cs == ERROR {
                    break 'out;
                }
                p += 1;
                if p == pe {
                    break 'out;
                }
            }
        }

        if !self.has_error() {
            self.cs = cs;
        }
        self.nread += p - off;
        out.done(buffer)
    }
}

/// The events of one `execute` as they are made.
#[derive(Default)]
struct Out {
    events: Vec<Event>,
    /// Each header name's [`Event::Field`], by its index in `events`.
    fields: HashMap<Vec<u8>, usize>,
    /// The first and past-the-last offsets `snake_upcase_char` ran on.
    upcased: Option<(usize, usize)>,
}

impl Out {
    /// `http_field`'s value for `name`: the first, or joined to the first.
    fn field(&mut self, name: &[u8], value: &[u8]) {
        if let Some(&at) = self.fields.get(name) {
            if let Event::Field(_, joined) = &mut self.events[at] {
                joined.extend_from_slice(b", ");
                joined.extend_from_slice(value);
            }
        } else {
            self.fields.insert(name.to_vec(), self.events.len());
            self.events
                .push(Event::Field(name.to_vec(), value.to_vec()));
        }
    }

    /// The events, a raise last.
    fn raised(mut self, long: Event, buffer: &[u8]) -> Vec<Event> {
        self.upcase(buffer);
        self.events.push(long);
        self.events
    }

    /// The events, done.
    fn done(mut self, buffer: &[u8]) -> Vec<Event> {
        self.upcase(buffer);
        self.events
    }

    fn upcase(&mut self, buffer: &[u8]) {
        if let Some((from, to)) = self.upcased.take() {
            self.events
                .push(Event::Upcase(from, buffer[from..to].to_vec()));
        }
    }
}

/// `VALIDATE_MAX_LENGTH`: the event that raises, when `len` is over.
fn too_long(len: usize, (name, max): (&'static str, usize)) -> Option<Event> {
    (len > max).then_some(Event::TooLong(name, max, len))
}

/// `snake_upcase_char`: capitalizes lower-case ASCII, turns dashes into
/// underscores and underscores into commas.
fn snake_upcase_char(c: &mut u8) {
    match *c {
        b'a'..=b'z' => *c &= !0x20,
        b'_' => *c = b',',
        b'-' => *c = b'_',
        _ => {}
    }
}

/// `http_field`'s trimming of `is_ows` bytes, spaces and tabs, from both
/// ends.
fn trim_ows(mut value: &[u8]) -> &[u8] {
    while let [rest @ .., b' ' | b'\t'] = value {
        value = rest;
    }
    while let [b' ' | b'\t', rest @ ..] = value {
        value = rest;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(parser: &mut Parser, text: &str) -> Vec<Event> {
        parser.execute(&mut text.as_bytes().to_vec(), 0)
    }

    #[test]
    fn a_request_line_and_headers() {
        let mut parser = Parser::new();
        let events = run(
            &mut parser,
            "GET /a/b?c=d#frag HTTP/1.1\r\nx-real_Ip:  1.2.3.4 \r\nHost: h\r\n\r\nbody",
        );
        assert!(parser.is_finished());
        assert!(!parser.has_error());
        assert_eq!(parser.nread(), 61);
        assert_eq!(
            events,
            vec![
                Event::Method(b"GET".to_vec()),
                Event::Path(b"/a/b".to_vec()),
                Event::Query(b"c=d".to_vec()),
                Event::Uri(b"/a/b?c=d".to_vec()),
                Event::Fragment(b"frag".to_vec()),
                Event::Protocol(b"HTTP/1.1".to_vec()),
                Event::Field(b"X_REAL,IP".to_vec(), b"1.2.3.4".to_vec()),
                Event::Field(b"HOST".to_vec(), b"h".to_vec()),
                Event::Body(b"body".to_vec()),
                Event::Upcase(28, b"X_REAL,IP:  1.2.3.4 \r\nHOST".to_vec()),
            ]
        );
    }

    #[test]
    fn a_request_in_pieces() {
        let mut parser = Parser::new();
        let whole = b"GET / HTTP/1.1\r\nHost: h\r\n\r\n".to_vec();
        let mut buffer = whole[..18].to_vec();
        let first = parser.execute(&mut buffer, 0);
        assert_eq!(parser.nread(), 18);
        assert!(!parser.is_finished());
        assert_eq!(first.last(), Some(&Event::Upcase(16, b"HO".to_vec())));
        buffer.extend_from_slice(&whole[18..]);
        let rest = parser.execute(&mut buffer, 18);
        assert!(parser.is_finished());
        assert_eq!(parser.nread(), whole.len());
        assert_eq!(
            rest,
            vec![
                Event::Field(b"HOST".to_vec(), b"h".to_vec()),
                Event::Body(Vec::new()),
                Event::Upcase(18, b"ST".to_vec()),
            ]
        );
    }

    #[test]
    fn duplicate_headers_are_joined_in_order() {
        let mut parser = Parser::new();
        let events = run(
            &mut parser,
            "GET / HTTP/1.1\r\nA: 1\r\nB: 2\r\na: 3\r\nA:4\r\n\r\n",
        );
        assert_eq!(
            events[4..6],
            [
                Event::Field(b"A".to_vec(), b"1, 3, 4".to_vec()),
                Event::Field(b"B".to_vec(), b"2".to_vec()),
            ]
        );
    }

    #[test]
    fn errors_stop_the_machine() {
        let mut parser = Parser::new();
        run(&mut parser, "GET / HTTP/1.1\r\nBad Header: x\r\n\r\n");
        assert!(parser.has_error());
        assert!(!parser.is_finished());
    }

    #[test]
    fn a_long_field_name_stops_where_the_c_raised() {
        let mut parser = Parser::new();
        let name = "A".repeat(257);
        let events = run(&mut parser, &format!("GET / HTTP/1.1\r\n{name}: x\r\n\r\n"));
        assert_eq!(events.last(), Some(&Event::TooLong("FIELD_NAME", 256, 257)));
        assert_eq!(parser.nread(), 0);
        assert!(!parser.has_error() && !parser.is_finished());
    }
}
