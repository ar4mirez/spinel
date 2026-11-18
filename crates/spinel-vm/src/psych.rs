//! libyaml for the built-in `psych` (#108): the parser and emitter that
//! Psych's C half (`psych_parser.c`, `psych_emitter.c`) drives, which
//! `lib/psych/spinel.rb` is Ruby over.
//!
//! The YAML is libyaml 0.2.5 itself, the release CRuby's Psych links,
//! through `unsafe-libyaml-norway`: libyaml's C translated line for line to
//! Rust, so the events, their marks, the errors and the emitter's bytes are
//! libyaml's. The crates written after libyaml's design rather than from its
//! code differ where a program can see it: `libyaml-safer` gives an implicit
//! document start the end mark of its first token, and the end of an
//! implicit flow mapping a column more than libyaml, and it panicked on one
//! yaml-test-suite case; this one matched CRuby's Psych on all 402 cases'
//! events and errors (`docs/engine.md`, "Built-in gems").
//!
//! Every libyaml call is here, behind a safe interface:
//!
//! - [`parse`] runs one parser over a whole input and answers its events, in
//!   order, and the syntax error that stopped it, if one did, as
//!   `psych_parser.c`'s `parse` would see them. The Ruby side hands each
//!   event to the handler. Given a depth limit, as it is when the handler
//!   builds objects, nesting deeper than it stops the parse with an error of
//!   its own; event parsing alone is unbounded, as libyaml's is.
//! - An [`Emitter`] is one `Psych::Emitter`'s `yaml_emitter_t`, kept in its
//!   heap's [`Emitters`] under an id never handed out twice. [`Emitters::emit`]
//!   emits one event and answers the bytes libyaml wrote, which the Ruby side
//!   writes to the emitter's IO, as `psych_emitter.c`'s `writer` does.

use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::mem::MaybeUninit;
use std::ptr::{self, NonNull};

// `sys`, as `openssl.rs` names its bindings: every call through it is in an
// `unsafe` block with its reason, which says what the name would.
#[allow(clippy::unsafe_removed_from_name)]
use unsafe_libyaml_norway as sys;

/// How deeply sequences and mappings may nest, when a parse builds objects,
/// before it stops, with an error, at the collection that went past it.
///
/// libyaml 0.2.5 has no such limit, and its parser keeps its states in a
/// heap stack, so a deep document parses, and a parse that only hands
/// events to a handler is unbounded here too; CRuby's Psych then raises
/// `SystemStackError` building the objects, a few thousand levels down. It
/// is well past what CRuby loads, so nothing CRuby loads is refused.
// ponytail: the ceiling is 10,000 collections when building objects. The
// interpreter's frames are on the heap rather than a C stack, so building a
// deeper document would recurse until memory runs out where CRuby raises
// `SystemStackError`. Upgrade: a recursion limit in the interpreter that
// raises `SystemStackError` as CRuby does, after which this bound can go.
pub const MAX_DEPTH: usize = 10_000;

/// Where an event starts or ends: libyaml's mark, without the index, which
/// Psych does not pass on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub line: u64,
    pub column: u64,
}

/// One parser event, as `psych_parser.c` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    StreamStart {
        encoding: i64,
    },
    StreamEnd,
    DocumentStart {
        version: Option<(i64, i64)>,
        /// Each `%TAG` directive's handle and prefix.
        tags: Vec<(Vec<u8>, Vec<u8>)>,
        implicit: bool,
    },
    DocumentEnd {
        implicit: bool,
    },
    Alias {
        anchor: Option<Vec<u8>>,
    },
    Scalar {
        value: Vec<u8>,
        anchor: Option<Vec<u8>>,
        tag: Option<Vec<u8>>,
        plain_implicit: bool,
        quoted_implicit: bool,
        style: i64,
    },
    SequenceStart {
        anchor: Option<Vec<u8>>,
        tag: Option<Vec<u8>>,
        implicit: bool,
        style: i64,
    },
    SequenceEnd,
    MappingStart {
        anchor: Option<Vec<u8>>,
        tag: Option<Vec<u8>>,
        implicit: bool,
        style: i64,
    },
    MappingEnd,
    /// `YAML_NO_EVENT`, which `psych_parser.c` answers with `empty`.
    Empty,
}

/// An event and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marked {
    pub event: Event,
    pub start: Mark,
    pub end: Mark,
    /// Where the parser had read to once it made the event, as `index`,
    /// `line` and `column`: what `Psych::Parser#mark` answers while the
    /// handler has it.
    pub parser: (u64, u64, u64),
}

/// `yaml_parser_t`'s fields up to and including `mark`, which is where its
/// reader is, in the same `repr(C)` layout. The crate keeps `mark`
/// crate-private and Psych's `Parser#mark` reads it, so it is read through
/// this. The size check below and `the_parser_mark_is_libyamls` (against
/// what CRuby's Psych answers) catch a release of the crate that moves it.
/// `yaml_read_handler_t`, as `ParserHead` lays it out.
type ReadHandler = unsafe fn(*mut c_void, *mut u8, u64, *mut u64) -> i32;

#[repr(C)]
struct ParserHead {
    error: u32,
    problem: *const u8,
    problem_offset: u64,
    problem_value: i32,
    problem_mark: [u64; 3],
    context: *const u8,
    context_mark: [u64; 3],
    read_handler: Option<ReadHandler>,
    read_handler_data: *mut c_void,
    input: [*const u8; 3],
    eof: bool,
    buffer: [*mut u8; 4],
    unread: u64,
    raw_buffer: [*mut u8; 4],
    encoding: u32,
    offset: u64,
    mark: [u64; 3],
}

const _: () =
    assert!(std::mem::size_of::<ParserHead>() <= std::mem::size_of::<sys::yaml_parser_t>());

/// Where `parser`'s reader is.
///
/// # Safety
///
/// `parser` is an initialised parser.
unsafe fn parser_mark(parser: *const sys::yaml_parser_t) -> (u64, u64, u64) {
    // SAFETY: `ParserHead` is a prefix of `yaml_parser_t`'s `repr(C)`
    // layout, and the parser is initialised.
    let [index, line, column] = unsafe { (*parser.cast::<ParserHead>()).mark };
    (index, line, column)
}

/// Why a parse stopped: what `make_exception` in `psych_parser.c` passes to
/// `Psych::SyntaxError.new`. The line and column are the context mark's,
/// one-based, as there, even for an error that has no context (whose mark
/// libyaml leaves at the start).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: u64,
    pub column: u64,
    pub offset: u64,
    pub problem: Option<Vec<u8>>,
    pub context: Option<Vec<u8>>,
}

/// A whole input's events, and the error that ended them early, if one did.
#[derive(Debug, Default)]
pub struct Parsed {
    pub events: Vec<Marked>,
    pub error: Option<SyntaxError>,
}

/// libyaml's encodings by the numbers `Psych::Parser::ANY` and the rest
/// name, which are libyaml's own.
fn encoding(code: i64) -> sys::yaml_encoding_t {
    match code {
        1 => sys::YAML_UTF8_ENCODING,
        2 => sys::YAML_UTF16LE_ENCODING,
        3 => sys::YAML_UTF16BE_ENCODING,
        _ => sys::YAML_ANY_ENCODING,
    }
}

/// The bytes of a NUL-terminated libyaml string, None for a null pointer.
///
/// # Safety
///
/// `p` is null or points at a NUL-terminated string.
unsafe fn c_bytes(p: *const u8) -> Option<Vec<u8>> {
    if p.is_null() {
        None
    } else {
        // SAFETY: the caller's.
        Some(unsafe { CStr::from_ptr(p.cast()) }.to_bytes().to_vec())
    }
}

fn mark(mark: sys::yaml_mark_t) -> Mark {
    Mark {
        line: mark.line,
        column: mark.column,
    }
}

/// Parse `input`, read as `encoding` (a `Psych::Parser` constant), to its
/// end or its first error: `psych_parser.c`'s loop, the handler aside.
/// Nesting deeper than `max_depth` collections is an error; 0 is no limit.
pub fn parse(input: &[u8], encoding_code: i64, max_depth: usize) -> Parsed {
    let mut parsed = Parsed::default();
    let mut parser = Box::new(MaybeUninit::<sys::yaml_parser_t>::uninit());
    let p = parser.as_mut_ptr();
    // SAFETY: `p` is this function's own parser, initialised before any
    // other call and deleted once at the end; the input outlives it.
    unsafe {
        if sys::yaml_parser_initialize(p).fail {
            parsed.error = Some(memory_error());
            return parsed;
        }
        sys::yaml_parser_set_encoding(p, encoding(encoding_code));
        sys::yaml_parser_set_input_string(p, input.as_ptr(), input.len() as u64);
    }
    let mut depth = 0usize;
    loop {
        let mut event = MaybeUninit::<sys::yaml_event_t>::uninit();
        // SAFETY: an initialised parser and an event for it to fill.
        if unsafe { sys::yaml_parser_parse(p, event.as_mut_ptr()) }.fail {
            // SAFETY: the parser is initialised; its error fields are set.
            parsed.error = Some(unsafe { syntax_error(&*p) });
            break;
        }
        // SAFETY: a successful parse filled the event in.
        let event = unsafe { event.assume_init_mut() };
        // SAFETY: the event libyaml just made, read by its type, then freed;
        // the parser it came from.
        let marked = unsafe {
            let mut marked = read_event(event);
            sys::yaml_event_delete(event);
            marked.parser = parser_mark(p);
            marked
        };
        match marked.event {
            Event::SequenceStart { .. } | Event::MappingStart { .. } => depth += 1,
            Event::SequenceEnd | Event::MappingEnd => depth = depth.saturating_sub(1),
            _ => {}
        }
        if max_depth != 0 && depth > max_depth {
            parsed.error = Some(SyntaxError {
                line: marked.start.line + 1,
                column: marked.start.column + 1,
                offset: 0,
                problem: Some(
                    format!("exceeded the maximum nesting depth of {max_depth}").into_bytes(),
                ),
                context: Some(b"while parsing a node".to_vec()),
            });
            break;
        }
        let done = marked.event == Event::StreamEnd;
        parsed.events.push(marked);
        if done {
            break;
        }
    }
    // SAFETY: initialised above and not used again.
    unsafe { sys::yaml_parser_delete(p) };
    parsed
}

/// What `make_exception` reads from a failed parser.
fn syntax_error(parser: &sys::yaml_parser_t) -> SyntaxError {
    if parser.error == sys::YAML_MEMORY_ERROR {
        return memory_error();
    }
    // SAFETY: libyaml's problem and context are null or static C strings.
    let (problem, context) = unsafe {
        (
            c_bytes(parser.problem.cast()),
            c_bytes(parser.context.cast()),
        )
    };
    SyntaxError {
        line: parser.context_mark.line + 1,
        column: parser.context_mark.column + 1,
        offset: parser.problem_offset,
        problem,
        context,
    }
}

/// libyaml out of memory, which Psych raises as `NoMemoryError`: no
/// problem, no context.
fn memory_error() -> SyntaxError {
    SyntaxError {
        line: 0,
        column: 0,
        offset: 0,
        problem: None,
        context: None,
    }
}

/// An event libyaml made, as owned data.
///
/// # Safety
///
/// `event` is one `yaml_parser_parse` filled in and has not been deleted.
unsafe fn read_event(event: &sys::yaml_event_t) -> Marked {
    let data = &event.data;
    // SAFETY: each arm reads the union member its event type sets, whose
    // strings are null or NUL-terminated, and a scalar's value is `length`
    // bytes.
    let event_data = unsafe {
        match event.type_ {
            sys::YAML_STREAM_START_EVENT => Event::StreamStart {
                encoding: data.stream_start.encoding as i64,
            },
            sys::YAML_STREAM_END_EVENT => Event::StreamEnd,
            sys::YAML_DOCUMENT_START_EVENT => {
                let start = data.document_start;
                let version = NonNull::new(start.version_directive)
                    .map(|v| (i64::from(v.as_ref().major), i64::from(v.as_ref().minor)));
                let mut tags = Vec::new();
                let mut tag = start.tag_directives.start;
                while !tag.is_null() && tag != start.tag_directives.end {
                    tags.push((
                        c_bytes((*tag).handle).unwrap_or_default(),
                        c_bytes((*tag).prefix).unwrap_or_default(),
                    ));
                    tag = tag.add(1);
                }
                Event::DocumentStart {
                    version,
                    tags,
                    implicit: start.implicit,
                }
            }
            sys::YAML_DOCUMENT_END_EVENT => Event::DocumentEnd {
                implicit: data.document_end.implicit,
            },
            sys::YAML_ALIAS_EVENT => Event::Alias {
                anchor: c_bytes(data.alias.anchor),
            },
            sys::YAML_SCALAR_EVENT => {
                let scalar = data.scalar;
                let value = if scalar.value.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(scalar.value, scalar.length as usize).to_vec()
                };
                Event::Scalar {
                    value,
                    anchor: c_bytes(scalar.anchor),
                    tag: c_bytes(scalar.tag),
                    plain_implicit: scalar.plain_implicit,
                    quoted_implicit: scalar.quoted_implicit,
                    style: scalar.style as i64,
                }
            }
            sys::YAML_SEQUENCE_START_EVENT => {
                let start = data.sequence_start;
                Event::SequenceStart {
                    anchor: c_bytes(start.anchor),
                    tag: c_bytes(start.tag),
                    implicit: start.implicit,
                    style: start.style as i64,
                }
            }
            sys::YAML_SEQUENCE_END_EVENT => Event::SequenceEnd,
            sys::YAML_MAPPING_START_EVENT => {
                let start = data.mapping_start;
                Event::MappingStart {
                    anchor: c_bytes(start.anchor),
                    tag: c_bytes(start.tag),
                    implicit: start.implicit,
                    style: start.style as i64,
                }
            }
            sys::YAML_MAPPING_END_EVENT => Event::MappingEnd,
            _ => Event::Empty,
        }
    };
    Marked {
        event: event_data,
        start: mark(event.start_mark),
        end: mark(event.end_mark),
        parser: (0, 0, 0),
    }
}

/// One event for an emitter, as `psych_emitter.c` builds it from what Ruby
/// passed. Anchors, tags and tag directives are C strings there, so a NUL
/// in one is refused on the Ruby side before it gets here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Emit {
    StreamStart {
        encoding: i64,
    },
    StreamEnd,
    DocumentStart {
        version: Option<(i32, i32)>,
        tags: Vec<(Vec<u8>, Vec<u8>)>,
        implicit: bool,
    },
    DocumentEnd {
        implicit: bool,
    },
    Alias {
        anchor: Option<Vec<u8>>,
    },
    Scalar {
        value: Vec<u8>,
        anchor: Option<Vec<u8>>,
        tag: Option<Vec<u8>>,
        plain_implicit: bool,
        quoted_implicit: bool,
        style: i64,
    },
    SequenceStart {
        anchor: Option<Vec<u8>>,
        tag: Option<Vec<u8>>,
        implicit: bool,
        style: i64,
    },
    SequenceEnd,
    MappingStart {
        anchor: Option<Vec<u8>>,
        tag: Option<Vec<u8>>,
        implicit: bool,
        style: i64,
    },
    MappingEnd,
}

/// A C string's bytes, NUL-terminated, for a libyaml call to copy.
fn c_string(bytes: &[u8]) -> Vec<u8> {
    let mut c = Vec::with_capacity(bytes.len() + 1);
    c.extend_from_slice(bytes);
    c.push(0);
    c
}

fn c_ptr(c: &Option<Vec<u8>>) -> *const u8 {
    c.as_ref().map_or(ptr::null(), |c| c.as_ptr())
}

fn scalar_style(style: i64) -> sys::yaml_scalar_style_t {
    match style {
        1 => sys::YAML_PLAIN_SCALAR_STYLE,
        2 => sys::YAML_SINGLE_QUOTED_SCALAR_STYLE,
        3 => sys::YAML_DOUBLE_QUOTED_SCALAR_STYLE,
        4 => sys::YAML_LITERAL_SCALAR_STYLE,
        5 => sys::YAML_FOLDED_SCALAR_STYLE,
        _ => sys::YAML_ANY_SCALAR_STYLE,
    }
}

fn sequence_style(style: i64) -> sys::yaml_sequence_style_t {
    match style {
        1 => sys::YAML_BLOCK_SEQUENCE_STYLE,
        2 => sys::YAML_FLOW_SEQUENCE_STYLE,
        _ => sys::YAML_ANY_SEQUENCE_STYLE,
    }
}

fn mapping_style(style: i64) -> sys::yaml_mapping_style_t {
    match style {
        1 => sys::YAML_BLOCK_MAPPING_STYLE,
        2 => sys::YAML_FLOW_MAPPING_STYLE,
        _ => sys::YAML_ANY_MAPPING_STYLE,
    }
}

/// libyaml's write handler: the bytes go to the emitter's own buffer, for
/// [`Emitters::emit`] to hand to Ruby.
///
/// # Safety
///
/// `data` is the `Vec<u8>` [`Emitter::new`] gave libyaml, and `buffer` is
/// `size` bytes.
unsafe fn write(data: *mut c_void, buffer: *mut u8, size: u64) -> i32 {
    // SAFETY: the caller's; libyaml calls this only inside `emit`, while
    // nothing else touches the buffer.
    unsafe {
        let out = &mut *data.cast::<Vec<u8>>();
        out.extend_from_slice(std::slice::from_raw_parts(buffer, size as usize));
    }
    1
}

/// One `Psych::Emitter`'s libyaml emitter and the bytes it has written and
/// Ruby has not yet taken.
pub struct Emitter {
    /// Boxed so libyaml's pointers into it stay put while the map moves it.
    raw: Box<MaybeUninit<sys::yaml_emitter_t>>,
    /// libyaml's write handler writes here. Only ever reached through this
    /// pointer, from `Box::into_raw`, and freed after the emitter is deleted.
    out: NonNull<Vec<u8>>,
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Emitter")
    }
}

impl Emitter {
    /// `psych_emitter.c`'s `allocate`: a libyaml emitter writing Unicode
    /// unescaped, indenting by two, to the emitter's buffer.
    fn new() -> Option<Emitter> {
        let mut raw = Box::new(MaybeUninit::<sys::yaml_emitter_t>::uninit());
        let out = NonNull::from(Box::leak(Box::new(Vec::new())));
        // SAFETY: a fresh emitter, initialised first; `out` lives as long as
        // the emitter (see `Drop`).
        unsafe {
            if sys::yaml_emitter_initialize(raw.as_mut_ptr()).fail {
                drop(Box::from_raw(out.as_ptr()));
                return None;
            }
            sys::yaml_emitter_set_unicode(raw.as_mut_ptr(), true);
            sys::yaml_emitter_set_indent(raw.as_mut_ptr(), 2);
            sys::yaml_emitter_set_output(raw.as_mut_ptr(), write, out.as_ptr().cast());
        }
        Some(Emitter { raw, out })
    }

    fn ptr(&mut self) -> *mut sys::yaml_emitter_t {
        self.raw.as_mut_ptr()
    }

    /// Emit `event`, answering what libyaml wrote, or its problem.
    fn emit(&mut self, event: &Emit) -> Result<Vec<u8>, Vec<u8>> {
        // Zeroed, so an event an `_initialize` refused (an anchor, tag or
        // value that is not UTF-8) is `YAML_NO_EVENT`, which the emitter
        // answers with the error of whatever it expected. `psych_emitter.c`
        // emits its uninitialised event the same way.
        let mut raw = MaybeUninit::<sys::yaml_event_t>::zeroed();
        let e = raw.as_mut_ptr();
        // SAFETY: each call copies the strings it is given, which live to
        // the end of the call; the event is the emitter's afterwards.
        let failed = unsafe {
            match event {
                Emit::StreamStart { encoding: code } => {
                    let _ = sys::yaml_stream_start_event_initialize(e, encoding(*code));
                }
                Emit::StreamEnd => {
                    let _ = sys::yaml_stream_end_event_initialize(e);
                }
                Emit::DocumentStart {
                    version,
                    tags,
                    implicit,
                } => {
                    let mut version = version.map(|(major, minor)| {
                        let mut v = MaybeUninit::<sys::yaml_version_directive_t>::zeroed();
                        let p = v.as_mut_ptr();
                        (*p).major = major;
                        (*p).minor = minor;
                        v.assume_init()
                    });
                    let strings: Vec<(Vec<u8>, Vec<u8>)> = tags
                        .iter()
                        .map(|(handle, prefix)| (c_string(handle), c_string(prefix)))
                        .collect();
                    let mut directives: Vec<sys::yaml_tag_directive_t> = strings
                        .iter()
                        .map(|(handle, prefix)| {
                            let mut d = MaybeUninit::<sys::yaml_tag_directive_t>::zeroed();
                            let p = d.as_mut_ptr();
                            (*p).handle = handle.as_ptr().cast_mut();
                            (*p).prefix = prefix.as_ptr().cast_mut();
                            d.assume_init()
                        })
                        .collect();
                    let range = directives.as_mut_ptr_range();
                    let (start, end) = if directives.is_empty() {
                        (ptr::null_mut(), ptr::null_mut())
                    } else {
                        (range.start, range.end)
                    };
                    let _ = sys::yaml_document_start_event_initialize(
                        e,
                        version.as_mut().map_or(ptr::null_mut(), ptr::from_mut),
                        start,
                        end,
                        *implicit,
                    );
                }
                Emit::DocumentEnd { implicit } => {
                    let _ = sys::yaml_document_end_event_initialize(e, *implicit);
                }
                Emit::Alias { anchor } => {
                    let anchor = anchor.as_deref().map(c_string);
                    if anchor.is_some() {
                        let _ = sys::yaml_alias_event_initialize(e, c_ptr(&anchor));
                    }
                }
                Emit::Scalar {
                    value,
                    anchor,
                    tag,
                    plain_implicit,
                    quoted_implicit,
                    style,
                } => {
                    let anchor = anchor.as_deref().map(c_string);
                    let tag = tag.as_deref().map(c_string);
                    // NUL-terminated as a Ruby String is: a length past a C
                    // int wraps negative, as `(int)RSTRING_LEN` does, and
                    // libyaml then takes the value's `strlen`.
                    let terminated = c_string(value);
                    let _ = sys::yaml_scalar_event_initialize(
                        e,
                        c_ptr(&anchor),
                        c_ptr(&tag),
                        terminated.as_ptr(),
                        value.len() as i32,
                        *plain_implicit,
                        *quoted_implicit,
                        scalar_style(*style),
                    );
                }
                Emit::SequenceStart {
                    anchor,
                    tag,
                    implicit,
                    style,
                } => {
                    let anchor = anchor.as_deref().map(c_string);
                    let tag = tag.as_deref().map(c_string);
                    let _ = sys::yaml_sequence_start_event_initialize(
                        e,
                        c_ptr(&anchor),
                        c_ptr(&tag),
                        *implicit,
                        sequence_style(*style),
                    );
                }
                Emit::SequenceEnd => {
                    let _ = sys::yaml_sequence_end_event_initialize(e);
                }
                Emit::MappingStart {
                    anchor,
                    tag,
                    implicit,
                    style,
                } => {
                    let anchor = anchor.as_deref().map(c_string);
                    let tag = tag.as_deref().map(c_string);
                    let _ = sys::yaml_mapping_start_event_initialize(
                        e,
                        c_ptr(&anchor),
                        c_ptr(&tag),
                        *implicit,
                        mapping_style(*style),
                    );
                }
                Emit::MappingEnd => {
                    let _ = sys::yaml_mapping_end_event_initialize(e);
                }
            }
            sys::yaml_emitter_emit(self.ptr(), e).fail
        };
        // SAFETY: `out` is only reached through this pointer, and libyaml is
        // done with it until the next `emit`.
        let written = std::mem::take(unsafe { &mut *self.out.as_ptr() });
        if failed {
            // SAFETY: an emitter whose emit failed has its problem set, a
            // static C string or null.
            let problem = unsafe {
                let emitter: &sys::yaml_emitter_t = &*self.ptr();
                c_bytes(emitter.problem.cast())
            };
            return Err(problem.unwrap_or_default());
        }
        Ok(written)
    }

    fn set(&mut self, setting: Setting) {
        // SAFETY: an initialised emitter.
        unsafe {
            match setting {
                Setting::Width(width) => sys::yaml_emitter_set_width(self.ptr(), width),
                Setting::Indent(indent) => sys::yaml_emitter_set_indent(self.ptr(), indent),
                Setting::Canonical(on) => sys::yaml_emitter_set_canonical(self.ptr(), on),
            }
        }
    }
}

impl Drop for Emitter {
    fn drop(&mut self) {
        // SAFETY: initialised in `new`, deleted once, here; then the buffer
        // its write handler pointed at, which nothing reaches any more.
        unsafe {
            sys::yaml_emitter_delete(self.raw.as_mut_ptr());
            drop(Box::from_raw(self.out.as_ptr()));
        }
    }
}

/// An emitter setting, as `yaml_emitter_set_width`, `_indent` and
/// `_canonical` take it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Width(i32),
    Indent(i32),
    Canonical(bool),
}

/// One heap's emitters, by id.
#[derive(Debug, Default)]
pub struct Emitters {
    live: HashMap<u64, Emitter>,
    next: u64,
}

impl Emitters {
    /// A new emitter's id, or None when libyaml could not allocate one.
    pub fn alloc(&mut self) -> Option<u64> {
        let emitter = Emitter::new()?;
        let id = self.next;
        self.next += 1;
        self.live.insert(id, emitter);
        Some(id)
    }

    /// Free emitter `id`; an id already freed is no emitter.
    pub fn free(&mut self, id: u64) {
        self.live.remove(&id);
    }

    /// Change a setting of emitter `id`; false when there is no such emitter.
    pub fn set(&mut self, id: u64, setting: Setting) -> bool {
        self.live.get_mut(&id).map(|e| e.set(setting)).is_some()
    }

    /// Emit `event` on emitter `id`: what it wrote, or libyaml's problem.
    /// None when there is no such emitter.
    pub fn emit(&mut self, id: u64, event: &Emit) -> Option<Result<Vec<u8>, Vec<u8>>> {
        self.live.get_mut(&id).map(|e| e.emit(event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(parsed: &Parsed) -> Vec<&'static str> {
        parsed
            .events
            .iter()
            .map(|m| match m.event {
                Event::StreamStart { .. } => "+STR",
                Event::StreamEnd => "-STR",
                Event::DocumentStart { .. } => "+DOC",
                Event::DocumentEnd { .. } => "-DOC",
                Event::Alias { .. } => "=ALI",
                Event::Scalar { .. } => "=VAL",
                Event::SequenceStart { .. } => "+SEQ",
                Event::SequenceEnd => "-SEQ",
                Event::MappingStart { .. } => "+MAP",
                Event::MappingEnd => "-MAP",
                Event::Empty => "NONE",
            })
            .collect()
    }

    #[test]
    fn parses_events_with_marks() {
        let parsed = parse(b"a: [1, &x b, *x]\n", 1, 0);
        assert!(parsed.error.is_none());
        assert_eq!(
            kinds(&parsed),
            [
                "+STR", "+DOC", "+MAP", "=VAL", "+SEQ", "=VAL", "=VAL", "=ALI", "-SEQ", "-MAP",
                "-DOC", "-STR"
            ]
        );
        let b = &parsed.events[6];
        assert_eq!(
            b.event,
            Event::Scalar {
                value: b"b".to_vec(),
                anchor: Some(b"x".to_vec()),
                tag: None,
                plain_implicit: true,
                quoted_implicit: false,
                style: 1,
            }
        );
        assert_eq!(
            (b.start, b.end),
            (
                Mark { line: 0, column: 7 },
                Mark {
                    line: 0,
                    column: 11
                }
            )
        );
    }

    #[test]
    fn the_parser_mark_is_libyamls() {
        // What CRuby's Psych (libyaml 0.2.5) answers for `parser.mark` in
        // each callback.
        let parsed = parse(b"a: [1, 2]\nb: {c: d}\n---\n- x\n", 1, 0);
        let marks: Vec<_> = parsed.events.iter().map(|m| m.parser).collect();
        assert_eq!(
            marks,
            [
                (0, 0, 0),
                (2, 0, 2),
                (2, 0, 2),
                (2, 0, 2),
                (4, 0, 4),
                (6, 0, 6),
                (9, 0, 9),
                (9, 0, 9),
                (12, 1, 2),
                (14, 1, 4),
                (16, 1, 6),
                (18, 1, 8),
                (19, 1, 9),
                (23, 2, 3),
                (23, 2, 3),
                (23, 2, 3),
                (25, 3, 1),
                (28, 4, 0),
                (28, 4, 0),
                (28, 4, 0),
                (28, 4, 0),
            ]
        );
    }

    #[test]
    fn a_syntax_error_reports_the_context_mark() {
        let parsed = parse(b"a: [1", 1, 0);
        let error = parsed.error.clone().expect("an error");
        assert_eq!((error.line, error.column), (1, 4));
        assert_eq!(
            error.problem.as_deref(),
            Some(&b"did not find expected ',' or ']'"[..])
        );
        assert_eq!(
            error.context.as_deref(),
            Some(&b"while parsing a flow sequence"[..])
        );
        // The events before the error are still there, as Psych's handler
        // would have had them.
        assert_eq!(
            kinds(&parsed),
            ["+STR", "+DOC", "+MAP", "=VAL", "+SEQ", "=VAL"]
        );
    }

    #[test]
    fn nesting_past_the_bound_stops_the_parse() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        let error = parse(deep.as_bytes(), 1, MAX_DEPTH)
            .error
            .expect("an error");
        assert_eq!((error.line, error.column), (1, MAX_DEPTH as u64 + 1));
        let shallow = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(shallow.as_bytes(), 1, MAX_DEPTH).error.is_none());
    }

    #[test]
    fn nesting_is_unbounded_without_a_limit() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        let parsed = parse(deep.as_bytes(), 1, 0);
        assert!(parsed.error.is_none());
        let starts = parsed
            .events
            .iter()
            .filter(|e| matches!(e.event, Event::SequenceStart { .. }))
            .count();
        assert_eq!(starts, MAX_DEPTH + 1);
    }

    #[test]
    fn emits_what_libyaml_writes() {
        let mut emitters = Emitters::default();
        let id = emitters.alloc().expect("an emitter");
        let mut out = Vec::new();
        let events = [
            Emit::StreamStart { encoding: 1 },
            Emit::DocumentStart {
                version: None,
                tags: Vec::new(),
                implicit: false,
            },
            Emit::MappingStart {
                anchor: None,
                tag: None,
                implicit: true,
                style: 1,
            },
            Emit::Scalar {
                value: b"a".to_vec(),
                anchor: None,
                tag: None,
                plain_implicit: true,
                quoted_implicit: false,
                style: 0,
            },
            Emit::Scalar {
                value: b"line\nbreak".to_vec(),
                anchor: None,
                tag: None,
                plain_implicit: true,
                quoted_implicit: true,
                style: 0,
            },
            Emit::MappingEnd,
            Emit::DocumentEnd { implicit: true },
            Emit::StreamEnd,
        ];
        for event in &events {
            out.extend(emitters.emit(id, event).expect("live").expect("emitted"));
        }
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "---\na: 'line\n\n  break'\n"
        );
        emitters.free(id);
        assert!(emitters.emit(id, &Emit::StreamEnd).is_none());
    }

    #[test]
    fn an_out_of_order_event_is_libyamls_problem() {
        let mut emitters = Emitters::default();
        let id = emitters.alloc().expect("an emitter");
        let problem = emitters
            .emit(id, &Emit::StreamEnd)
            .expect("live")
            .expect_err("refused");
        assert_eq!(problem, b"expected STREAM-START");
    }
}
