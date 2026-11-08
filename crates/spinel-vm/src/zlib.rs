//! zlib 1.3.1, ported to Rust, behind `lib/zlib.rb` (#59).
//!
//! CRuby's `zlib` gem is `ext/zlib/zlib.c` over the system's zlib. Its C half
//! is Ruby here (`lib/zlib.rb`); what Ruby cannot say is zlib itself, and that
//! is this file: `deflate.c`, `trees.c`, `inflate.c`, `inftrees.c`,
//! `inffast.c`, `adler32.c` and `crc32.c`, function by function. A port and not
//! a crate because what a program sees is zlib's bytes: `Zlib.deflate` is
//! compared byte for byte by ruby/spec, a gzip file's bytes are a checksum
//! some program keeps, and a DataError's message is zlib's `strm->msg`. A
//! crate's deflate is a different encoder of the same format.
//!
//! A stream's state lives in a Ruby String, as `mt.rs` keeps a generator's,
//! so a heap owns it and nothing here is global: [`deflate_init`] and
//! [`inflate_init`] answer its bytes, and every other call reads and rewrites
//! them where they lie. The small fields are decoded on entry and encoded on
//! exit; the window, hash chains and pending buffer are borrowed in place,
//! so a call costs what zlib's own does plus a few kilobytes of copying.
//!
//! Inflate reads untrusted bytes. Nothing here panics on them: every index a
//! hostile stream can steer is masked or bounds-checked as zlib's own C is,
//! and a call writes at most the `avail_out` it is given, which bounds its
//! memory. A state String whose bytes are not one this file wrote is refused
//! (`None`) rather than trusted.

// ---------------------------------------------------------------------------
// The C API's numbers.

pub const Z_OK: i32 = 0;
pub const Z_STREAM_END: i32 = 1;
pub const Z_NEED_DICT: i32 = 2;
pub const Z_STREAM_ERROR: i32 = -2;
pub const Z_DATA_ERROR: i32 = -3;
pub const Z_MEM_ERROR: i32 = -4;
pub const Z_BUF_ERROR: i32 = -5;

pub const Z_NO_FLUSH: i32 = 0;
pub const Z_PARTIAL_FLUSH: i32 = 1;
pub const Z_SYNC_FLUSH: i32 = 2;
pub const Z_FULL_FLUSH: i32 = 3;
pub const Z_FINISH: i32 = 4;
pub const Z_BLOCK: i32 = 5;
pub const Z_TREES: i32 = 6;

const Z_FILTERED: i32 = 1;
const Z_HUFFMAN_ONLY: i32 = 2;
const Z_RLE: i32 = 3;
const Z_FIXED: i32 = 4;

const Z_BINARY: i32 = 0;
const Z_TEXT: i32 = 1;
const Z_UNKNOWN: i32 = 2;

const Z_DEFLATED: i32 = 8;

/// What `strm->msg` can say, by the index a state keeps. Index 0 is NULL.
const MESSAGES: [&str; 22] = [
    "",
    "stream error",
    "buffer error",
    "insufficient memory",
    "incorrect header check",
    "unknown compression method",
    "invalid window size",
    "unknown header flags set",
    "header crc mismatch",
    "invalid block type",
    "invalid stored block lengths",
    "too many length or distance symbols",
    "invalid code lengths set",
    "invalid bit length repeat",
    "invalid code -- missing end-of-block",
    "invalid literal/lengths set",
    "invalid distances set",
    "invalid literal/length code",
    "invalid distance code",
    "invalid distance too far back",
    "incorrect data check",
    "incorrect length check",
];

mod msg {
    pub const STREAM_ERROR: u8 = 1;
    pub const BUF_ERROR: u8 = 2;
    pub const HEADER_CHECK: u8 = 4;
    pub const METHOD: u8 = 5;
    pub const WINDOW_SIZE: u8 = 6;
    pub const HEADER_FLAGS: u8 = 7;
    pub const HEADER_CRC: u8 = 8;
    pub const BLOCK_TYPE: u8 = 9;
    pub const STORED_LENGTHS: u8 = 10;
    pub const TOO_MANY_SYMBOLS: u8 = 11;
    pub const CODE_LENGTHS: u8 = 12;
    pub const BIT_LENGTH_REPEAT: u8 = 13;
    pub const MISSING_END: u8 = 14;
    pub const LITLEN_SET: u8 = 15;
    pub const DISTANCES_SET: u8 = 16;
    pub const LITLEN_CODE: u8 = 17;
    pub const DISTANCE_CODE: u8 = 18;
    pub const TOO_FAR: u8 = 19;
    pub const DATA_CHECK: u8 = 20;
    pub const LENGTH_CHECK: u8 = 21;
}

/// The message a state's `msg` index stands for, or None for NULL.
pub fn message(index: u8) -> Option<&'static str> {
    match index {
        0 => None,
        n => MESSAGES.get(n as usize).copied(),
    }
}

/// `zError`: what Ruby says for an error the stream left no message for.
pub fn error_text(err: i32) -> &'static str {
    match err {
        Z_STREAM_END => "stream end",
        Z_NEED_DICT => "need dictionary",
        Z_OK => "",
        -1 => "file error",
        Z_STREAM_ERROR => "stream error",
        Z_DATA_ERROR => "data error",
        Z_MEM_ERROR => "insufficient memory",
        Z_BUF_ERROR => "buffer error",
        -6 => "incompatible version",
        _ => "",
    }
}

/// `z_stream`, the part of it that is the caller's: the input this call may
/// read, the output it writes (`out`, at most `avail_out` more bytes), and
/// the counters the stream keeps.
pub struct Strm<'a> {
    pub input: &'a [u8],
    pub next_in: usize,
    pub out: Vec<u8>,
    pub avail_out: u32,
    /// `next_out == Z_NULL`: Ruby's stream after its buffer was taken.
    pub null_out: bool,
    pub total_in: u64,
    pub total_out: u64,
    pub adler: u64,
    pub data_type: i32,
    pub msg: u8,
}

impl<'a> Strm<'a> {
    pub fn new(input: &'a [u8], avail_out: u32) -> Strm<'a> {
        // `MAX_UINT`: zlib's counts are 32 bits.
        let input = &input[..input.len().min(u32::MAX as usize)];
        Strm {
            input,
            next_in: 0,
            out: Vec::with_capacity(avail_out.min(1 << 16) as usize),
            avail_out,
            null_out: false,
            total_in: 0,
            total_out: 0,
            adler: 0,
            data_type: 0,
            msg: 0,
        }
    }

    fn avail_in(&self) -> u32 {
        (self.input.len() - self.next_in) as u32
    }
}

// ---------------------------------------------------------------------------
// adler32.c and crc32.c.

const BASE: u64 = 65521;
const NMAX: usize = 5552;

/// `adler32_z`, None for `buf == Z_NULL`.
pub fn adler32(adler: u64, buf: Option<&[u8]>) -> u64 {
    let mut sum2 = (adler >> 16) & 0xffff;
    let mut adler = adler & 0xffff;
    let Some(buf) = buf else {
        return 1;
    };
    if buf.len() == 1 {
        adler += u64::from(buf[0]);
        if adler >= BASE {
            adler -= BASE;
        }
        sum2 += adler;
        if sum2 >= BASE {
            sum2 -= BASE;
        }
        return adler | (sum2 << 16);
    }
    if buf.len() < 16 {
        for &b in buf {
            adler += u64::from(b);
            sum2 += adler;
        }
        if adler >= BASE {
            adler -= BASE;
        }
        sum2 %= BASE;
        return adler | (sum2 << 16);
    }
    let mut chunks = buf.chunks_exact(NMAX);
    for chunk in &mut chunks {
        for &b in chunk {
            adler += u64::from(b);
            sum2 += adler;
        }
        adler %= BASE;
        sum2 %= BASE;
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        for &b in rest {
            adler += u64::from(b);
            sum2 += adler;
        }
        adler %= BASE;
        sum2 %= BASE;
    }
    adler | (sum2 << 16)
}

const POLY: u32 = 0xedb8_8320;

const fn make_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut p = i as u32;
        let mut j = 0;
        while j < 8 {
            p = if p & 1 != 0 { (p >> 1) ^ POLY } else { p >> 1 };
            j += 1;
        }
        table[i] = p;
        i += 1;
    }
    table
}

/// `get_crc_table`.
pub const CRC_TABLE: [u32; 256] = make_crc_table();

/// `crc32_z`, None for `buf == Z_NULL`. The braided and hardware forms
/// compute the same CRC; this is the byte-at-a-time one.
pub fn crc32(crc: u64, buf: Option<&[u8]>) -> u64 {
    let Some(buf) = buf else {
        return 0;
    };
    let mut c = !(crc as u32);
    for &b in buf {
        c = (c >> 8) ^ CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize];
    }
    u64::from(!c)
}

// ---------------------------------------------------------------------------
// trees.c: the static tables, computed at compile time as `tr_static_init`
// computes them.

const LENGTH_CODES: usize = 29;
const LITERALS: usize = 256;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const MAX_BL_BITS: usize = 7;
const BUF_SIZE: i32 = 16;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;
const STORED_BLOCK: u32 = 0;
const STATIC_TREES: u32 = 1;
const DYN_TREES: u32 = 2;

const MIN_MATCH: u32 = 3;
const MAX_MATCH: u32 = 258;
const MIN_LOOKAHEAD: u32 = MAX_MATCH + MIN_MATCH + 1;
const WIN_INIT: u64 = MAX_MATCH as u64;
const TOO_FAR: u32 = 4096;
const MAX_STORED: u32 = 65535;

const EXTRA_LBITS: [u32; LENGTH_CODES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const EXTRA_DBITS: [u32; D_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BLBITS: [u32; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// `ct_data`: `fc` is Freq or Code, `dl` is Dad or Len.
#[derive(Clone, Copy, Default)]
struct Ct {
    fc: u16,
    dl: u16,
}

struct Static {
    ltree: [Ct; L_CODES + 2],
    dtree: [Ct; D_CODES],
    dist_code: [u8; 512],
    length_code: [u8; 256],
    base_length: [u32; LENGTH_CODES],
    base_dist: [u32; D_CODES],
}

const fn bi_reverse(mut code: u32, mut len: i32) -> u32 {
    let mut res = 0;
    loop {
        res |= code & 1;
        code >>= 1;
        res <<= 1;
        len -= 1;
        if len <= 0 {
            break;
        }
    }
    res >> 1
}

const fn build_static() -> Static {
    let mut t = Static {
        ltree: [Ct { fc: 0, dl: 0 }; L_CODES + 2],
        dtree: [Ct { fc: 0, dl: 0 }; D_CODES],
        dist_code: [0; 512],
        length_code: [0; 256],
        base_length: [0; LENGTH_CODES],
        base_dist: [0; D_CODES],
    };
    let mut length = 0usize;
    let mut code = 0usize;
    while code < LENGTH_CODES - 1 {
        t.base_length[code] = length as u32;
        let mut n = 0;
        while n < (1 << EXTRA_LBITS[code]) {
            t.length_code[length] = code as u8;
            length += 1;
            n += 1;
        }
        code += 1;
    }
    t.length_code[length - 1] = code as u8;
    let mut dist = 0usize;
    code = 0;
    while code < 16 {
        t.base_dist[code] = dist as u32;
        let mut n = 0;
        while n < (1 << EXTRA_DBITS[code]) {
            t.dist_code[dist] = code as u8;
            dist += 1;
            n += 1;
        }
        code += 1;
    }
    dist >>= 7;
    while code < D_CODES {
        t.base_dist[code] = (dist << 7) as u32;
        let mut n = 0;
        while n < (1 << (EXTRA_DBITS[code] - 7)) {
            t.dist_code[256 + dist] = code as u8;
            dist += 1;
            n += 1;
        }
        code += 1;
    }
    let mut bl_count = [0u16; MAX_BITS + 1];
    let mut n = 0;
    while n <= 143 {
        t.ltree[n].dl = 8;
        bl_count[8] += 1;
        n += 1;
    }
    while n <= 255 {
        t.ltree[n].dl = 9;
        bl_count[9] += 1;
        n += 1;
    }
    while n <= 279 {
        t.ltree[n].dl = 7;
        bl_count[7] += 1;
        n += 1;
    }
    while n <= 287 {
        t.ltree[n].dl = 8;
        bl_count[8] += 1;
        n += 1;
    }
    // gen_codes(static_ltree, L_CODES + 1, bl_count)
    let mut next_code = [0u16; MAX_BITS + 1];
    let mut c = 0u32;
    let mut bits = 1;
    while bits <= MAX_BITS {
        c = (c + bl_count[bits - 1] as u32) << 1;
        next_code[bits] = c as u16;
        bits += 1;
    }
    n = 0;
    while n <= L_CODES + 1 {
        let len = t.ltree[n].dl as usize;
        if len != 0 {
            t.ltree[n].fc = bi_reverse(next_code[len] as u32, len as i32) as u16;
            next_code[len] += 1;
        }
        n += 1;
    }
    n = 0;
    while n < D_CODES {
        t.dtree[n].dl = 5;
        t.dtree[n].fc = bi_reverse(n as u32, 5) as u16;
        n += 1;
    }
    t
}

const TABLES: Static = build_static();

fn d_code(dist: u32) -> usize {
    if dist < 256 {
        TABLES.dist_code[dist as usize] as usize
    } else {
        TABLES.dist_code[(256 + (dist >> 7) as usize).min(511)] as usize
    }
}

/// `static_tree_desc`, by which tree it describes.
#[derive(Clone, Copy, PartialEq)]
enum Tree {
    L,
    D,
    Bl,
}

// ---------------------------------------------------------------------------
// The deflate state: deflate.h's `internal_state`, its scalars kept in the
// state String's header and its arrays where they lie.

const INIT_STATE: i32 = 42;
const GZIP_STATE: i32 = 57;
const BUSY_STATE: i32 = 113;
const FINISH_STATE: i32 = 666;

/// One of `configuration_table`'s compress functions.
#[derive(Clone, Copy, PartialEq)]
enum Func {
    Stored,
    Fast,
    Slow,
}

struct Config {
    good_length: u32,
    max_lazy: u32,
    nice_length: i32,
    max_chain: u32,
    func: Func,
}

const fn cfg(
    good_length: u32,
    max_lazy: u32,
    nice_length: i32,
    max_chain: u32,
    func: Func,
) -> Config {
    Config {
        good_length,
        max_lazy,
        nice_length,
        max_chain,
        func,
    }
}

const CONFIGURATION: [Config; 10] = [
    cfg(0, 0, 0, 0, Func::Stored),
    cfg(4, 4, 8, 4, Func::Fast),
    cfg(4, 5, 16, 8, Func::Fast),
    cfg(4, 6, 32, 32, Func::Fast),
    cfg(4, 4, 16, 16, Func::Slow),
    cfg(8, 16, 32, 32, Func::Slow),
    cfg(8, 16, 128, 128, Func::Slow),
    cfg(8, 32, 128, 256, Func::Slow),
    cfg(32, 128, 258, 1024, Func::Slow),
    cfg(32, 258, 258, 4096, Func::Slow),
];

fn config(level: i32) -> &'static Config {
    &CONFIGURATION[level.clamp(0, 9) as usize]
}

fn rank(f: i32) -> i32 {
    f * 2 - if f > 4 { 9 } else { 0 }
}

#[derive(Clone, Copy, PartialEq)]
enum BlockState {
    NeedMore,
    BlockDone,
    FinishStarted,
    FinishDone,
}

/// The scalar fields, each kept as eight little-endian bytes.
macro_rules! scalars {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Clone, Copy, Default)]
        struct $name { $($field: $ty),* }

        impl $name {
            const COUNT: usize = [$(stringify!($field)),*].len();

            fn read(bytes: &[u8]) -> Self {
                let mut s = Self::default();
                let mut at = 0;
                $(
                    let mut word = [0u8; 8];
                    word.copy_from_slice(&bytes[at..at + 8]);
                    s.$field = i64::from_le_bytes(word) as $ty;
                    at += 8;
                )*
                let _ = at;
                s
            }

            fn write(&self, bytes: &mut [u8]) {
                let mut at = 0;
                $(
                    bytes[at..at + 8].copy_from_slice(&(self.$field as i64).to_le_bytes());
                    at += 8;
                )*
                let _ = at;
            }
        }
    };
}

scalars!(DeflateScalars {
    status: i32,
    pending: u32,
    pending_out: u32,
    wrap: i32,
    last_flush: i32,
    w_bits: u32,
    w_size: u32,
    w_mask: u32,
    window_size: u64,
    ins_h: u32,
    hash_bits: u32,
    hash_size: u32,
    hash_mask: u32,
    hash_shift: u32,
    block_start: i64,
    match_length: u32,
    prev_match: u32,
    match_available: i32,
    strstart: u32,
    match_start: u32,
    lookahead: u32,
    prev_length: u32,
    max_chain_length: u32,
    max_lazy_match: u32,
    level: i32,
    strategy: i32,
    good_match: u32,
    nice_match: i32,
    lit_bufsize: u32,
    pending_buf_size: u64,
    sym_next: u32,
    sym_end: u32,
    opt_len: u64,
    static_len: u64,
    matches: u32,
    insert: u32,
    bi_buf: u16,
    bi_valid: i32,
    high_water: u64,
    mem_level: u32,
    total_in: u64,
    total_out: u64,
    adler: u64,
    data_type: i32,
    msg: u8,
});

const DEFLATE_MAGIC: &[u8; 8] = b"SpZlDef1";
const TREE_BYTES: usize = (HEAP_SIZE + 2 * D_CODES + 1 + 2 * BL_CODES + 1) * 4;
const DEFLATE_HEADER: usize = 8 + DeflateScalars::COUNT * 8 + TREE_BYTES;

fn deflate_size(w_bits: u32, mem_level: u32) -> usize {
    let w_size = 1usize << w_bits;
    let hash_size = 1usize << (mem_level + 7);
    let lit_bufsize = 1usize << (mem_level + 6);
    DEFLATE_HEADER + 2 * w_size + 2 * w_size + 2 * hash_size + 4 * lit_bufsize
}

fn get16(bytes: &[u8], index: usize) -> u16 {
    match bytes.get(2 * index..2 * index + 2) {
        Some(pair) => u16::from_le_bytes([pair[0], pair[1]]),
        None => 0,
    }
}

fn set16(bytes: &mut [u8], index: usize, value: u16) {
    if let Some(pair) = bytes.get_mut(2 * index..2 * index + 2) {
        pair.copy_from_slice(&value.to_le_bytes());
    }
}

struct Deflate<'b> {
    s: DeflateScalars,
    ltree: [Ct; HEAP_SIZE],
    dtree: [Ct; 2 * D_CODES + 1],
    bltree: [Ct; 2 * BL_CODES + 1],
    l_max_code: i32,
    d_max_code: i32,
    bl_max_code: i32,
    bl_count: [u16; MAX_BITS + 1],
    heap: [i32; 2 * L_CODES + 1],
    heap_len: i32,
    heap_max: i32,
    depth: [u8; 2 * L_CODES + 1],
    header: &'b mut [u8],
    window: &'b mut [u8],
    prev: &'b mut [u8],
    head: &'b mut [u8],
    pbuf: &'b mut [u8],
}

fn read_trees(bytes: &[u8], trees: &mut [&mut [Ct]]) {
    let mut at = 0;
    for tree in trees.iter_mut() {
        for ct in tree.iter_mut() {
            ct.fc = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
            ct.dl = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]);
            at += 4;
        }
    }
}

fn write_trees(bytes: &mut [u8], trees: &[&[Ct]]) {
    let mut at = 0;
    for tree in trees {
        for ct in tree.iter() {
            bytes[at..at + 2].copy_from_slice(&ct.fc.to_le_bytes());
            bytes[at + 2..at + 4].copy_from_slice(&ct.dl.to_le_bytes());
            at += 4;
        }
    }
}

impl<'b> Deflate<'b> {
    /// The state these bytes are, or None when they are not one this file
    /// wrote: the wrong length, or fields no deflate state could hold.
    fn open(bytes: &'b mut [u8]) -> Option<Deflate<'b>> {
        if bytes.len() < DEFLATE_HEADER || &bytes[..8] != DEFLATE_MAGIC {
            return None;
        }
        let s = DeflateScalars::read(&bytes[8..]);
        if !(9..=15).contains(&s.w_bits) || !(1..=9).contains(&s.mem_level) {
            return None;
        }
        if bytes.len() != deflate_size(s.w_bits, s.mem_level) {
            return None;
        }
        let w_size = 1u32 << s.w_bits;
        let hash_bits = s.mem_level + 7;
        let lit_bufsize = 1u32 << (s.mem_level + 6);
        let window_size = 2 * u64::from(w_size);
        let sane = s.w_size == w_size
            && s.w_mask == w_size - 1
            && s.hash_bits == hash_bits
            && s.hash_size == 1 << hash_bits
            && s.hash_mask == (1 << hash_bits) - 1
            && s.hash_shift == hash_bits.div_ceil(MIN_MATCH)
            && s.lit_bufsize == lit_bufsize
            && s.pending_buf_size == u64::from(lit_bufsize) * 4
            && s.sym_end == (lit_bufsize - 1) * 3
            && s.sym_next <= s.sym_end
            && u64::from(s.pending_out) + u64::from(s.pending) <= s.pending_buf_size
            && (s.window_size == 0 || s.window_size == window_size)
            && u64::from(s.strstart) + u64::from(s.lookahead) <= window_size
            && s.block_start <= i64::from(s.strstart)
            && s.insert <= w_size
            && s.ins_h <= s.hash_mask
            && s.prev_length <= MAX_MATCH
            && s.match_length <= MAX_MATCH
            && (0..=9).contains(&s.level)
            && (0..=Z_FIXED).contains(&s.strategy)
            && (0..=BUF_SIZE).contains(&s.bi_valid)
            && s.high_water <= window_size
            && (s.msg as usize) < MESSAGES.len();
        if !sane {
            return None;
        }
        let (header, rest) = bytes.split_at_mut(DEFLATE_HEADER);
        let (window, rest) = rest.split_at_mut(2 * w_size as usize);
        let (prev, rest) = rest.split_at_mut(2 * w_size as usize);
        let (head, pbuf) = rest.split_at_mut(2 * (1usize << hash_bits));
        let mut d = Deflate {
            s,
            ltree: [Ct::default(); HEAP_SIZE],
            dtree: [Ct::default(); 2 * D_CODES + 1],
            bltree: [Ct::default(); 2 * BL_CODES + 1],
            l_max_code: 0,
            d_max_code: 0,
            bl_max_code: 0,
            bl_count: [0; MAX_BITS + 1],
            heap: [0; 2 * L_CODES + 1],
            heap_len: 0,
            heap_max: 0,
            depth: [0; 2 * L_CODES + 1],
            header,
            window,
            prev,
            head,
            pbuf,
        };
        let at = 8 + DeflateScalars::COUNT * 8;
        let (lt, dt, bt) = (&mut d.ltree, &mut d.dtree, &mut d.bltree);
        read_trees(
            &d.header[at..],
            &mut [&mut lt[..], &mut dt[..], &mut bt[..]],
        );
        Some(d)
    }

    fn save(&mut self) {
        self.s.write(&mut self.header[8..]);
        let at = 8 + DeflateScalars::COUNT * 8;
        write_trees(
            &mut self.header[at..],
            &[&self.ltree[..], &self.dtree[..], &self.bltree[..]],
        );
    }

    fn load_strm(&self, z: &mut Strm) {
        z.total_in = self.s.total_in;
        z.total_out = self.s.total_out;
        z.adler = self.s.adler;
        z.data_type = self.s.data_type;
        z.msg = self.s.msg;
    }

    fn store_strm(&mut self, z: &Strm) {
        self.s.total_in = z.total_in;
        self.s.total_out = z.total_out;
        self.s.adler = z.adler;
        self.s.data_type = z.data_type;
        self.s.msg = z.msg;
    }

    fn max_dist(&self) -> u32 {
        self.s.w_size - MIN_LOOKAHEAD
    }

    fn win(&self, at: usize) -> u8 {
        self.window.get(at).copied().unwrap_or(0)
    }

    fn put_byte(&mut self, c: u8) {
        if let Some(b) = self.pbuf.get_mut(self.s.pending as usize) {
            *b = c;
        }
        self.s.pending += 1;
    }

    fn put_short(&mut self, w: u16) {
        self.put_byte((w & 0xff) as u8);
        self.put_byte((w >> 8) as u8);
    }

    fn put_short_msb(&mut self, b: u32) {
        self.put_byte((b >> 8) as u8);
        self.put_byte((b & 0xff) as u8);
    }

    fn update_hash(&mut self, c: u8) {
        self.s.ins_h = ((self.s.ins_h << self.s.hash_shift) ^ u32::from(c)) & self.s.hash_mask;
    }

    fn insert_string(&mut self, str: u32) -> u32 {
        let c = self.win(str as usize + (MIN_MATCH as usize - 1));
        self.update_hash(c);
        let h = self.s.ins_h as usize;
        let match_head = get16(self.head, h);
        set16(self.prev, (str & self.s.w_mask) as usize, match_head);
        set16(self.head, h, str as u16);
        u32::from(match_head)
    }

    fn clear_hash(&mut self) {
        self.head.fill(0);
    }

    fn slide_hash(&mut self) {
        let wsize = self.s.w_size;
        for n in 0..self.s.hash_size as usize {
            let m = u32::from(get16(self.head, n));
            set16(
                self.head,
                n,
                if m >= wsize { (m - wsize) as u16 } else { 0 },
            );
        }
        for n in 0..wsize as usize {
            let m = u32::from(get16(self.prev, n));
            set16(
                self.prev,
                n,
                if m >= wsize { (m - wsize) as u16 } else { 0 },
            );
        }
    }

    /// `read_buf`: take up to `size` bytes of input, checksummed as the
    /// wrapper asks. Answers where they are in `z.input`.
    fn read_buf(&mut self, z: &mut Strm, size: u32) -> (usize, usize) {
        let len = z.avail_in().min(size) as usize;
        if len == 0 {
            return (z.next_in, 0);
        }
        let start = z.next_in;
        let data = &z.input[start..start + len];
        if self.s.wrap == 1 {
            z.adler = adler32(z.adler, Some(data));
        } else if self.s.wrap == 2 {
            z.adler = crc32(z.adler, Some(data));
        }
        z.next_in += len;
        z.total_in += len as u64;
        (start, len)
    }

    fn fill_window(&mut self, z: &mut Strm) {
        let wsize = self.s.w_size;
        let input = z.input;
        loop {
            let mut more = (self.s.window_size
                - u64::from(self.s.lookahead)
                - u64::from(self.s.strstart)) as u32;
            if self.s.strstart >= wsize + self.max_dist() {
                let n = (wsize - more) as usize;
                let from = wsize as usize;
                if from + n <= self.window.len() {
                    self.window.copy_within(from..from + n, 0);
                }
                self.s.match_start = self.s.match_start.wrapping_sub(wsize);
                self.s.strstart -= wsize;
                self.s.block_start -= i64::from(wsize);
                if self.s.insert > self.s.strstart {
                    self.s.insert = self.s.strstart;
                }
                self.slide_hash();
                more += wsize;
            }
            if z.avail_in() == 0 {
                break;
            }
            let at = (self.s.strstart + self.s.lookahead) as usize;
            let (start, n) = self.read_buf(z, more);
            if let Some(dst) = self.window.get_mut(at..at + n) {
                dst.copy_from_slice(&input[start..start + n]);
            }
            self.s.lookahead += n as u32;
            if self.s.lookahead + self.s.insert >= MIN_MATCH {
                let mut str = self.s.strstart.saturating_sub(self.s.insert);
                self.s.ins_h = u32::from(self.win(str as usize));
                let c = self.win(str as usize + 1);
                self.update_hash(c);
                while self.s.insert != 0 {
                    let c = self.win(str as usize + MIN_MATCH as usize - 1);
                    self.update_hash(c);
                    let h = self.s.ins_h as usize;
                    let head = get16(self.head, h);
                    set16(self.prev, (str & self.s.w_mask) as usize, head);
                    set16(self.head, h, str as u16);
                    str += 1;
                    self.s.insert -= 1;
                    if self.s.lookahead + self.s.insert < MIN_MATCH {
                        break;
                    }
                }
            }
            if !(self.s.lookahead < MIN_LOOKAHEAD && z.avail_in() != 0) {
                break;
            }
        }
        if self.s.high_water < self.s.window_size {
            let curr = u64::from(self.s.strstart) + u64::from(self.s.lookahead);
            if self.s.high_water < curr {
                let init = (self.s.window_size - curr).min(WIN_INIT);
                self.zero_window(curr as usize, init as usize);
                self.s.high_water = curr + init;
            } else if self.s.high_water < curr + WIN_INIT {
                let init = (curr + WIN_INIT - self.s.high_water)
                    .min(self.s.window_size - self.s.high_water);
                self.zero_window(self.s.high_water as usize, init as usize);
                self.s.high_water += init;
            }
        }
    }

    fn zero_window(&mut self, at: usize, len: usize) {
        if let Some(part) = self.window.get_mut(at..at + len) {
            part.fill(0);
        }
    }

    fn lm_init(&mut self) {
        self.s.window_size = 2 * u64::from(self.s.w_size);
        self.clear_hash();
        let c = config(self.s.level);
        self.s.max_lazy_match = c.max_lazy;
        self.s.good_match = c.good_length;
        self.s.nice_match = c.nice_length;
        self.s.max_chain_length = c.max_chain;
        self.s.strstart = 0;
        self.s.block_start = 0;
        self.s.lookahead = 0;
        self.s.insert = 0;
        self.s.match_length = MIN_MATCH - 1;
        self.s.prev_length = MIN_MATCH - 1;
        self.s.match_available = 0;
        self.s.ins_h = 0;
    }

    fn reset_keep(&mut self, z: &mut Strm) {
        z.total_in = 0;
        z.total_out = 0;
        z.msg = 0;
        z.data_type = Z_UNKNOWN;
        self.s.pending = 0;
        self.s.pending_out = 0;
        if self.s.wrap < 0 {
            self.s.wrap = -self.s.wrap;
        }
        self.s.status = if self.s.wrap == 2 {
            GZIP_STATE
        } else {
            INIT_STATE
        };
        z.adler = if self.s.wrap == 2 {
            crc32(0, None)
        } else {
            adler32(0, None)
        };
        self.s.last_flush = -2;
        self.tr_init();
    }

    fn reset(&mut self, z: &mut Strm) {
        self.reset_keep(z);
        self.lm_init();
    }

    fn set_dictionary(&mut self, z: &mut Strm, dictionary: &[u8]) -> i32 {
        let wrap = self.s.wrap;
        if wrap == 2 || (wrap == 1 && self.s.status != INIT_STATE) || self.s.lookahead != 0 {
            return Z_STREAM_ERROR;
        }
        let mut dictionary = &dictionary[..dictionary.len().min(u32::MAX as usize)];
        if wrap == 1 {
            z.adler = adler32(z.adler, Some(dictionary));
        }
        self.s.wrap = 0;
        let w_size = self.s.w_size as usize;
        if dictionary.len() >= w_size {
            if wrap == 0 {
                self.clear_hash();
                self.s.strstart = 0;
                self.s.block_start = 0;
                self.s.insert = 0;
            }
            dictionary = &dictionary[dictionary.len() - w_size..];
        }
        let mut dz = Strm::new(dictionary, 0);
        dz.total_in = z.total_in;
        dz.adler = z.adler;
        self.fill_window(&mut dz);
        while self.s.lookahead >= MIN_MATCH {
            let mut str = self.s.strstart;
            let mut n = self.s.lookahead - (MIN_MATCH - 1);
            loop {
                let c = self.win(str as usize + MIN_MATCH as usize - 1);
                self.update_hash(c);
                let h = self.s.ins_h as usize;
                let head = get16(self.head, h);
                set16(self.prev, (str & self.s.w_mask) as usize, head);
                set16(self.head, h, str as u16);
                str += 1;
                n -= 1;
                if n == 0 {
                    break;
                }
            }
            self.s.strstart = str;
            self.s.lookahead = MIN_MATCH - 1;
            self.fill_window(&mut dz);
        }
        z.total_in = dz.total_in;
        self.s.strstart += self.s.lookahead;
        self.s.block_start = i64::from(self.s.strstart);
        self.s.insert = self.s.lookahead;
        self.s.lookahead = 0;
        self.s.match_length = MIN_MATCH - 1;
        self.s.prev_length = MIN_MATCH - 1;
        self.s.match_available = 0;
        self.s.wrap = wrap;
        Z_OK
    }

    fn params(&mut self, z: &mut Strm, level: i32, strategy: i32) -> i32 {
        let level = if level == -1 { 6 } else { level };
        if !(0..=9).contains(&level) || !(0..=Z_FIXED).contains(&strategy) {
            return Z_STREAM_ERROR;
        }
        let func = config(self.s.level).func;
        if (strategy != self.s.strategy || func != config(level).func) && self.s.last_flush != -2 {
            let err = self.deflate(z, Z_BLOCK);
            if err == Z_STREAM_ERROR {
                return err;
            }
            if z.avail_in() != 0
                || (i64::from(self.s.strstart) - self.s.block_start) + i64::from(self.s.lookahead)
                    != 0
            {
                return Z_BUF_ERROR;
            }
        }
        if self.s.level != level {
            if self.s.level == 0 && self.s.matches != 0 {
                if self.s.matches == 1 {
                    self.slide_hash();
                } else {
                    self.clear_hash();
                }
                self.s.matches = 0;
            }
            self.s.level = level;
            let c = config(level);
            self.s.max_lazy_match = c.max_lazy;
            self.s.good_match = c.good_length;
            self.s.nice_match = c.nice_length;
            self.s.max_chain_length = c.max_chain;
        }
        self.s.strategy = strategy;
        Z_OK
    }

    fn flush_pending(&mut self, z: &mut Strm) {
        self.bi_flush();
        let len = self.s.pending.min(z.avail_out);
        if len == 0 {
            return;
        }
        let start = self.s.pending_out as usize;
        if let Some(bytes) = self.pbuf.get(start..start + len as usize) {
            z.out.extend_from_slice(bytes);
        }
        self.s.pending_out += len;
        z.total_out += u64::from(len);
        z.avail_out -= len;
        self.s.pending -= len;
        if self.s.pending == 0 {
            self.s.pending_out = 0;
        }
    }

    fn err_return(z: &mut Strm, err: i32) -> i32 {
        z.msg = match err {
            Z_STREAM_ERROR => msg::STREAM_ERROR,
            Z_BUF_ERROR => msg::BUF_ERROR,
            _ => 0,
        };
        err
    }

    fn deflate(&mut self, z: &mut Strm, flush: i32) -> i32 {
        if !(0..=Z_BLOCK).contains(&flush) {
            return Z_STREAM_ERROR;
        }
        if z.null_out || (self.s.status == FINISH_STATE && flush != Z_FINISH) {
            return Self::err_return(z, Z_STREAM_ERROR);
        }
        if z.avail_out == 0 {
            return Self::err_return(z, Z_BUF_ERROR);
        }
        let old_flush = self.s.last_flush;
        self.s.last_flush = flush;
        if self.s.pending != 0 {
            self.flush_pending(z);
            if z.avail_out == 0 {
                self.s.last_flush = -1;
                return Z_OK;
            }
        } else if z.avail_in() == 0 && rank(flush) <= rank(old_flush) && flush != Z_FINISH {
            return Self::err_return(z, Z_BUF_ERROR);
        }
        if self.s.status == FINISH_STATE && z.avail_in() != 0 {
            return Self::err_return(z, Z_BUF_ERROR);
        }
        if self.s.status == INIT_STATE && self.s.wrap == 0 {
            self.s.status = BUSY_STATE;
        }
        if self.s.status == INIT_STATE {
            let mut header = (Z_DEFLATED as u32 + ((self.s.w_bits - 8) << 4)) << 8;
            let level_flags = if self.s.strategy >= Z_HUFFMAN_ONLY || self.s.level < 2 {
                0
            } else if self.s.level < 6 {
                1
            } else if self.s.level == 6 {
                2
            } else {
                3
            };
            header |= level_flags << 6;
            if self.s.strstart != 0 {
                header |= 0x20;
            }
            header += 31 - (header % 31);
            self.put_short_msb(header);
            if self.s.strstart != 0 {
                self.put_short_msb((z.adler >> 16) as u32 & 0xffff);
                self.put_short_msb((z.adler & 0xffff) as u32);
            }
            z.adler = adler32(0, None);
            self.s.status = BUSY_STATE;
            self.flush_pending(z);
            if self.s.pending != 0 {
                self.s.last_flush = -1;
                return Z_OK;
            }
        }
        if self.s.status == GZIP_STATE {
            // No gz_header: Ruby never sets one.
            z.adler = crc32(0, None);
            self.put_byte(31);
            self.put_byte(139);
            self.put_byte(8);
            for _ in 0..5 {
                self.put_byte(0);
            }
            let xfl = if self.s.level == 9 {
                2
            } else if self.s.strategy >= Z_HUFFMAN_ONLY || self.s.level < 2 {
                4
            } else {
                0
            };
            self.put_byte(xfl);
            self.put_byte(3); // OS_CODE, Unix
            self.s.status = BUSY_STATE;
            self.flush_pending(z);
            if self.s.pending != 0 {
                self.s.last_flush = -1;
                return Z_OK;
            }
        }
        if z.avail_in() != 0
            || self.s.lookahead != 0
            || (flush != Z_NO_FLUSH && self.s.status != FINISH_STATE)
        {
            let bstate = if self.s.level == 0 {
                self.deflate_stored(z, flush)
            } else if self.s.strategy == Z_HUFFMAN_ONLY {
                self.deflate_huff(z, flush)
            } else if self.s.strategy == Z_RLE {
                self.deflate_rle(z, flush)
            } else if config(self.s.level).func == Func::Fast {
                self.deflate_fast(z, flush)
            } else {
                self.deflate_slow(z, flush)
            };
            if bstate == BlockState::FinishStarted || bstate == BlockState::FinishDone {
                self.s.status = FINISH_STATE;
            }
            if bstate == BlockState::NeedMore || bstate == BlockState::FinishStarted {
                if z.avail_out == 0 {
                    self.s.last_flush = -1;
                }
                return Z_OK;
            }
            if bstate == BlockState::BlockDone {
                if flush == Z_PARTIAL_FLUSH {
                    self.tr_align();
                } else if flush != Z_BLOCK {
                    self.tr_stored_block(None, 0, 0);
                    if flush == Z_FULL_FLUSH {
                        self.clear_hash();
                        if self.s.lookahead == 0 {
                            self.s.strstart = 0;
                            self.s.block_start = 0;
                            self.s.insert = 0;
                        }
                    }
                }
                self.flush_pending(z);
                if z.avail_out == 0 {
                    self.s.last_flush = -1;
                    return Z_OK;
                }
            }
        }
        if flush != Z_FINISH {
            return Z_OK;
        }
        if self.s.wrap <= 0 {
            return Z_STREAM_END;
        }
        if self.s.wrap == 2 {
            let a = z.adler;
            let t = z.total_in;
            for shift in [0, 8, 16, 24] {
                self.put_byte((a >> shift) as u8);
            }
            for shift in [0, 8, 16, 24] {
                self.put_byte((t >> shift) as u8);
            }
        } else {
            self.put_short_msb((z.adler >> 16) as u32 & 0xffff);
            self.put_short_msb((z.adler & 0xffff) as u32);
        }
        self.flush_pending(z);
        if self.s.wrap > 0 {
            self.s.wrap = -self.s.wrap;
        }
        if self.s.pending != 0 {
            Z_OK
        } else {
            Z_STREAM_END
        }
    }

    fn longest_match(&mut self, mut cur_match: u32) -> u32 {
        let mut chain_length = self.s.max_chain_length;
        let scan = self.s.strstart as usize;
        let mut best_len = self.s.prev_length as usize;
        let mut nice_match = self.s.nice_match as usize;
        let max_dist = self.max_dist();
        let limit = self.s.strstart.saturating_sub(max_dist);
        let wmask = self.s.w_mask;
        let strend = scan + MAX_MATCH as usize;
        let w = &*self.window;
        if strend >= w.len() || best_len == 0 {
            return self.s.prev_length.min(self.s.lookahead);
        }
        let mut scan_end1 = w[scan + best_len - 1];
        let mut scan_end = w[scan + best_len];
        if self.s.prev_length >= self.s.good_match {
            chain_length >>= 2;
        }
        if nice_match > self.s.lookahead as usize {
            nice_match = self.s.lookahead as usize;
        }
        let mut match_start = self.s.match_start;
        loop {
            let m = cur_match as usize;
            if m + MAX_MATCH as usize >= w.len() {
                break;
            }
            if !(w[m + best_len] != scan_end
                || w[m + best_len - 1] != scan_end1
                || w[m] != w[scan]
                || w[m + 1] != w[scan + 1])
            {
                // scan += 2, match++; then compare until a mismatch or strend,
                // eight at a time as the C's unrolled loop checks strend.
                let mut sp = scan + 2;
                let mut mp = m + 2;
                'outer: loop {
                    for _ in 0..8 {
                        sp += 1;
                        mp += 1;
                        if w[sp] != w[mp] {
                            break 'outer;
                        }
                    }
                    if sp >= strend {
                        break;
                    }
                }
                let len = MAX_MATCH as usize - (strend - sp);
                if len > best_len {
                    match_start = cur_match;
                    best_len = len;
                    if len >= nice_match {
                        break;
                    }
                    scan_end1 = w[scan + best_len - 1];
                    scan_end = w[scan + best_len];
                }
            }
            cur_match = u32::from(get16(self.prev, (cur_match & wmask) as usize));
            if cur_match <= limit {
                break;
            }
            chain_length -= 1;
            if chain_length == 0 {
                break;
            }
        }
        self.s.match_start = match_start;
        if best_len as u32 <= self.s.lookahead {
            best_len as u32
        } else {
            self.s.lookahead
        }
    }

    fn flush_block_only(&mut self, z: &mut Strm, last: u32) {
        let buf = if self.s.block_start >= 0 {
            Some(self.s.block_start as usize)
        } else {
            None
        };
        let stored_len = (i64::from(self.s.strstart) - self.s.block_start) as u64;
        self.tr_flush_block(z, buf, stored_len, last);
        self.s.block_start = i64::from(self.s.strstart);
        self.flush_pending(z);
    }

    fn deflate_stored(&mut self, z: &mut Strm, flush: i32) -> BlockState {
        let input = z.input;
        let mut min_block = ((self.s.pending_buf_size - 5) as u32).min(self.s.w_size);
        let mut last = 0u32;
        let mut used = z.avail_in();
        loop {
            let mut len = MAX_STORED;
            let mut have = ((self.s.bi_valid + 42) >> 3) as u32;
            if z.avail_out < have {
                break;
            }
            have = z.avail_out - have;
            let mut left = (i64::from(self.s.strstart) - self.s.block_start) as u32;
            if u64::from(len) > u64::from(left) + u64::from(z.avail_in()) {
                len = left + z.avail_in();
            }
            if len > have {
                len = have;
            }
            if len < min_block
                && ((len == 0 && flush != Z_FINISH)
                    || flush == Z_NO_FLUSH
                    || len != left + z.avail_in())
            {
                break;
            }
            last = if flush == Z_FINISH && len == left + z.avail_in() {
                1
            } else {
                0
            };
            self.tr_stored_block(None, 0, last);
            let p = self.s.pending as usize;
            for (k, byte) in [len as u8, (len >> 8) as u8, !len as u8, (!len >> 8) as u8]
                .into_iter()
                .enumerate()
            {
                if let Some(slot) = p.checked_sub(4 - k).and_then(|i| self.pbuf.get_mut(i)) {
                    *slot = byte;
                }
            }
            self.flush_pending(z);
            if left != 0 {
                if left > len {
                    left = len;
                }
                let from = self.s.block_start as usize;
                if let Some(bytes) = self.window.get(from..from + left as usize) {
                    z.out.extend_from_slice(bytes);
                }
                z.avail_out -= left;
                z.total_out += u64::from(left);
                self.s.block_start += i64::from(left);
                len -= left;
            }
            if len != 0 {
                let (start, n) = self.read_buf(z, len);
                z.out.extend_from_slice(&input[start..start + n]);
                z.avail_out -= len;
                z.total_out += u64::from(len);
            }
            if last != 0 {
                break;
            }
        }
        used -= z.avail_in();
        if used != 0 {
            let w_size = self.s.w_size;
            if used >= w_size {
                self.s.matches = 2;
                let end = z.next_in;
                if let (Some(src), Some(dst)) = (
                    input.get(end - w_size as usize..end),
                    self.window.get_mut(..w_size as usize),
                ) {
                    dst.copy_from_slice(src);
                }
                self.s.strstart = w_size;
                self.s.insert = self.s.strstart;
            } else {
                if self.s.window_size - u64::from(self.s.strstart) <= u64::from(used) {
                    self.s.strstart -= w_size;
                    let n = self.s.strstart as usize;
                    self.window
                        .copy_within(w_size as usize..w_size as usize + n, 0);
                    if self.s.matches < 2 {
                        self.s.matches += 1;
                    }
                    if self.s.insert > self.s.strstart {
                        self.s.insert = self.s.strstart;
                    }
                }
                let at = self.s.strstart as usize;
                let end = z.next_in;
                if let (Some(src), Some(dst)) = (
                    input.get(end - used as usize..end),
                    self.window.get_mut(at..at + used as usize),
                ) {
                    dst.copy_from_slice(src);
                }
                self.s.strstart += used;
                self.s.insert += used.min(w_size - self.s.insert);
            }
            self.s.block_start = i64::from(self.s.strstart);
        }
        if self.s.high_water < u64::from(self.s.strstart) {
            self.s.high_water = u64::from(self.s.strstart);
        }
        if last != 0 {
            return BlockState::FinishDone;
        }
        if flush != Z_NO_FLUSH
            && flush != Z_FINISH
            && z.avail_in() == 0
            && i64::from(self.s.strstart) == self.s.block_start
        {
            return BlockState::BlockDone;
        }
        let mut have = (self.s.window_size - u64::from(self.s.strstart)) as u32;
        if z.avail_in() > have && self.s.block_start >= i64::from(self.s.w_size) {
            self.s.block_start -= i64::from(self.s.w_size);
            self.s.strstart -= self.s.w_size;
            let n = self.s.strstart as usize;
            let w = self.s.w_size as usize;
            self.window.copy_within(w..w + n, 0);
            if self.s.matches < 2 {
                self.s.matches += 1;
            }
            have += self.s.w_size;
            if self.s.insert > self.s.strstart {
                self.s.insert = self.s.strstart;
            }
        }
        if have > z.avail_in() {
            have = z.avail_in();
        }
        if have != 0 {
            let at = self.s.strstart as usize;
            let (start, n) = self.read_buf(z, have);
            if let Some(dst) = self.window.get_mut(at..at + n) {
                dst.copy_from_slice(&input[start..start + n]);
            }
            self.s.strstart += have;
            self.s.insert += have.min(self.s.w_size - self.s.insert);
        }
        if self.s.high_water < u64::from(self.s.strstart) {
            self.s.high_water = u64::from(self.s.strstart);
        }
        have = ((self.s.bi_valid + 42) >> 3) as u32;
        have = ((self.s.pending_buf_size - u64::from(have)) as u32).min(MAX_STORED);
        min_block = have.min(self.s.w_size);
        let left = (i64::from(self.s.strstart) - self.s.block_start) as u32;
        if left >= min_block
            || ((left != 0 || flush == Z_FINISH)
                && flush != Z_NO_FLUSH
                && z.avail_in() == 0
                && left <= have)
        {
            let len = left.min(have);
            last = if flush == Z_FINISH && z.avail_in() == 0 && len == left {
                1
            } else {
                0
            };
            let from = self.s.block_start as usize;
            self.tr_stored_block(Some(from), u64::from(len), last);
            self.s.block_start += i64::from(len);
            self.flush_pending(z);
        }
        if last != 0 {
            BlockState::FinishStarted
        } else {
            BlockState::NeedMore
        }
    }

    fn tally_lit(&mut self, c: u8) -> bool {
        let at = self.s.lit_bufsize as usize + self.s.sym_next as usize;
        for (k, b) in [0, 0, c].into_iter().enumerate() {
            if let Some(slot) = self.pbuf.get_mut(at + k) {
                *slot = b;
            }
        }
        self.s.sym_next += 3;
        self.ltree[c as usize].fc = self.ltree[c as usize].fc.wrapping_add(1);
        self.s.sym_next == self.s.sym_end
    }

    fn tally_dist(&mut self, distance: u32, length: u32) -> bool {
        let len = length as u8;
        let dist = distance as u16;
        let at = self.s.lit_bufsize as usize + self.s.sym_next as usize;
        for (k, b) in [dist as u8, (dist >> 8) as u8, len].into_iter().enumerate() {
            if let Some(slot) = self.pbuf.get_mut(at + k) {
                *slot = b;
            }
        }
        self.s.sym_next += 3;
        let dist = u32::from(dist.wrapping_sub(1));
        let l = TABLES.length_code[len as usize] as usize + LITERALS + 1;
        self.ltree[l].fc = self.ltree[l].fc.wrapping_add(1);
        let d = d_code(dist);
        self.dtree[d].fc = self.dtree[d].fc.wrapping_add(1);
        self.s.sym_next == self.s.sym_end
    }

    fn deflate_fast(&mut self, z: &mut Strm, flush: i32) -> BlockState {
        loop {
            if self.s.lookahead < MIN_LOOKAHEAD {
                self.fill_window(z);
                if self.s.lookahead < MIN_LOOKAHEAD && flush == Z_NO_FLUSH {
                    return BlockState::NeedMore;
                }
                if self.s.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = 0;
            if self.s.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.s.strstart);
            }
            if hash_head != 0 && self.s.strstart - hash_head <= self.max_dist() {
                self.s.match_length = self.longest_match(hash_head);
            }
            let bflush;
            if self.s.match_length >= MIN_MATCH {
                bflush = self.tally_dist(
                    self.s.strstart - self.s.match_start,
                    self.s.match_length - MIN_MATCH,
                );
                self.s.lookahead -= self.s.match_length;
                if self.s.match_length <= self.s.max_lazy_match && self.s.lookahead >= MIN_MATCH {
                    self.s.match_length -= 1;
                    loop {
                        self.s.strstart += 1;
                        self.insert_string(self.s.strstart);
                        self.s.match_length -= 1;
                        if self.s.match_length == 0 {
                            break;
                        }
                    }
                    self.s.strstart += 1;
                } else {
                    self.s.strstart += self.s.match_length;
                    self.s.match_length = 0;
                    self.s.ins_h = u32::from(self.win(self.s.strstart as usize));
                    let c = self.win(self.s.strstart as usize + 1);
                    self.update_hash(c);
                }
            } else {
                let c = self.win(self.s.strstart as usize);
                bflush = self.tally_lit(c);
                self.s.lookahead -= 1;
                self.s.strstart += 1;
            }
            if bflush {
                self.flush_block_only(z, 0);
                if z.avail_out == 0 {
                    return BlockState::NeedMore;
                }
            }
        }
        self.finish_blocks(
            z,
            flush,
            if self.s.strstart < MIN_MATCH - 1 {
                self.s.strstart
            } else {
                MIN_MATCH - 1
            },
        )
    }

    /// The end every compress function but `deflate_stored` shares.
    fn finish_blocks(&mut self, z: &mut Strm, flush: i32, insert: u32) -> BlockState {
        self.s.insert = insert;
        if flush == Z_FINISH {
            self.flush_block_only(z, 1);
            if z.avail_out == 0 {
                return BlockState::FinishStarted;
            }
            return BlockState::FinishDone;
        }
        if self.s.sym_next != 0 {
            self.flush_block_only(z, 0);
            if z.avail_out == 0 {
                return BlockState::NeedMore;
            }
        }
        BlockState::BlockDone
    }

    fn deflate_slow(&mut self, z: &mut Strm, flush: i32) -> BlockState {
        loop {
            if self.s.lookahead < MIN_LOOKAHEAD {
                self.fill_window(z);
                if self.s.lookahead < MIN_LOOKAHEAD && flush == Z_NO_FLUSH {
                    return BlockState::NeedMore;
                }
                if self.s.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = 0;
            if self.s.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.s.strstart);
            }
            self.s.prev_length = self.s.match_length;
            self.s.prev_match = self.s.match_start;
            self.s.match_length = MIN_MATCH - 1;
            if hash_head != 0
                && self.s.prev_length < self.s.max_lazy_match
                && self.s.strstart - hash_head <= self.max_dist()
            {
                self.s.match_length = self.longest_match(hash_head);
                if self.s.match_length <= 5
                    && (self.s.strategy == Z_FILTERED
                        || (self.s.match_length == MIN_MATCH
                            && self.s.strstart.wrapping_sub(self.s.match_start) > TOO_FAR))
                {
                    self.s.match_length = MIN_MATCH - 1;
                }
            }
            if self.s.prev_length >= MIN_MATCH && self.s.match_length <= self.s.prev_length {
                let max_insert = self.s.strstart + self.s.lookahead - MIN_MATCH;
                let bflush = self.tally_dist(
                    self.s.strstart - 1 - self.s.prev_match,
                    self.s.prev_length - MIN_MATCH,
                );
                self.s.lookahead -= self.s.prev_length - 1;
                self.s.prev_length -= 2;
                loop {
                    self.s.strstart += 1;
                    if self.s.strstart <= max_insert {
                        self.insert_string(self.s.strstart);
                    }
                    self.s.prev_length -= 1;
                    if self.s.prev_length == 0 {
                        break;
                    }
                }
                self.s.match_available = 0;
                self.s.match_length = MIN_MATCH - 1;
                self.s.strstart += 1;
                if bflush {
                    self.flush_block_only(z, 0);
                    if z.avail_out == 0 {
                        return BlockState::NeedMore;
                    }
                }
            } else if self.s.match_available != 0 {
                let c = self.win(self.s.strstart as usize - 1);
                if self.tally_lit(c) {
                    self.flush_block_only(z, 0);
                }
                self.s.strstart += 1;
                self.s.lookahead -= 1;
                if z.avail_out == 0 {
                    return BlockState::NeedMore;
                }
            } else {
                self.s.match_available = 1;
                self.s.strstart += 1;
                self.s.lookahead -= 1;
            }
        }
        if self.s.match_available != 0 {
            let c = self.win(self.s.strstart as usize - 1);
            self.tally_lit(c);
            self.s.match_available = 0;
        }
        self.finish_blocks(
            z,
            flush,
            if self.s.strstart < MIN_MATCH - 1 {
                self.s.strstart
            } else {
                MIN_MATCH - 1
            },
        )
    }

    fn deflate_rle(&mut self, z: &mut Strm, flush: i32) -> BlockState {
        loop {
            if self.s.lookahead <= MAX_MATCH {
                self.fill_window(z);
                if self.s.lookahead <= MAX_MATCH && flush == Z_NO_FLUSH {
                    return BlockState::NeedMore;
                }
                if self.s.lookahead == 0 {
                    break;
                }
            }
            self.s.match_length = 0;
            if self.s.lookahead >= MIN_MATCH && self.s.strstart > 0 {
                let mut scan = self.s.strstart as usize - 1;
                let prev = self.win(scan);
                if prev == self.win(scan + 1)
                    && prev == self.win(scan + 2)
                    && prev == self.win(scan + 3)
                {
                    scan += 3;
                    let strend = self.s.strstart as usize + MAX_MATCH as usize;
                    'outer: loop {
                        for _ in 0..8 {
                            scan += 1;
                            if prev != self.win(scan) {
                                break 'outer;
                            }
                        }
                        if scan >= strend {
                            break;
                        }
                    }
                    self.s.match_length = MAX_MATCH - (strend - scan) as u32;
                    if self.s.match_length > self.s.lookahead {
                        self.s.match_length = self.s.lookahead;
                    }
                }
            }
            let bflush;
            if self.s.match_length >= MIN_MATCH {
                bflush = self.tally_dist(1, self.s.match_length - MIN_MATCH);
                self.s.lookahead -= self.s.match_length;
                self.s.strstart += self.s.match_length;
                self.s.match_length = 0;
            } else {
                let c = self.win(self.s.strstart as usize);
                bflush = self.tally_lit(c);
                self.s.lookahead -= 1;
                self.s.strstart += 1;
            }
            if bflush {
                self.flush_block_only(z, 0);
                if z.avail_out == 0 {
                    return BlockState::NeedMore;
                }
            }
        }
        self.finish_blocks(z, flush, 0)
    }

    fn deflate_huff(&mut self, z: &mut Strm, flush: i32) -> BlockState {
        loop {
            if self.s.lookahead == 0 {
                self.fill_window(z);
                if self.s.lookahead == 0 {
                    if flush == Z_NO_FLUSH {
                        return BlockState::NeedMore;
                    }
                    break;
                }
            }
            self.s.match_length = 0;
            let c = self.win(self.s.strstart as usize);
            let bflush = self.tally_lit(c);
            self.s.lookahead -= 1;
            self.s.strstart += 1;
            if bflush {
                self.flush_block_only(z, 0);
                if z.avail_out == 0 {
                    return BlockState::NeedMore;
                }
            }
        }
        self.finish_blocks(z, flush, 0)
    }

    // --- trees.c ------------------------------------------------------------

    fn tr_init(&mut self) {
        self.s.bi_buf = 0;
        self.s.bi_valid = 0;
        self.init_block();
    }

    fn init_block(&mut self) {
        for n in 0..L_CODES {
            self.ltree[n].fc = 0;
        }
        for n in 0..D_CODES {
            self.dtree[n].fc = 0;
        }
        for n in 0..BL_CODES {
            self.bltree[n].fc = 0;
        }
        self.ltree[END_BLOCK].fc = 1;
        self.s.opt_len = 0;
        self.s.static_len = 0;
        self.s.sym_next = 0;
        self.s.matches = 0;
    }

    fn send_bits(&mut self, value: u32, length: i32) {
        let value = value & 0xffff;
        if self.s.bi_valid > BUF_SIZE - length {
            self.s.bi_buf |= (value << self.s.bi_valid) as u16;
            let b = self.s.bi_buf;
            self.put_short(b);
            self.s.bi_buf = (value >> (BUF_SIZE - self.s.bi_valid)) as u16;
            self.s.bi_valid += length - BUF_SIZE;
        } else {
            self.s.bi_buf |= (value << self.s.bi_valid) as u16;
            self.s.bi_valid += length;
        }
    }

    fn send_code(&mut self, c: usize, tree: Tree) {
        let ct = match tree {
            Tree::L => self.ltree[c],
            Tree::D => self.dtree[c],
            Tree::Bl => self.bltree[c],
        };
        self.send_bits(u32::from(ct.fc), i32::from(ct.dl));
    }

    fn bi_flush(&mut self) {
        if self.s.bi_valid == 16 {
            let b = self.s.bi_buf;
            self.put_short(b);
            self.s.bi_buf = 0;
            self.s.bi_valid = 0;
        } else if self.s.bi_valid >= 8 {
            self.put_byte(self.s.bi_buf as u8);
            self.s.bi_buf >>= 8;
            self.s.bi_valid -= 8;
        }
    }

    fn bi_windup(&mut self) {
        if self.s.bi_valid > 8 {
            let b = self.s.bi_buf;
            self.put_short(b);
        } else if self.s.bi_valid > 0 {
            self.put_byte(self.s.bi_buf as u8);
        }
        self.s.bi_buf = 0;
        self.s.bi_valid = 0;
    }

    fn tree(&mut self, which: Tree) -> &mut [Ct] {
        match which {
            Tree::L => &mut self.ltree,
            Tree::D => &mut self.dtree,
            Tree::Bl => &mut self.bltree,
        }
    }

    fn smaller(tree: &[Ct], depth: &[u8], n: usize, m: usize) -> bool {
        tree[n].fc < tree[m].fc || (tree[n].fc == tree[m].fc && depth[n] <= depth[m])
    }

    fn pqdownheap(&mut self, which: Tree, mut k: usize) {
        let v = self.heap[k] as usize;
        let mut j = k << 1;
        let heap_len = self.heap_len as usize;
        loop {
            if j > heap_len {
                break;
            }
            let (tree, depth): (&[Ct], &[u8]) = match which {
                Tree::L => (&self.ltree, &self.depth),
                Tree::D => (&self.dtree, &self.depth),
                Tree::Bl => (&self.bltree, &self.depth),
            };
            if j < heap_len
                && Self::smaller(
                    tree,
                    depth,
                    self.heap[j + 1] as usize,
                    self.heap[j] as usize,
                )
            {
                j += 1;
            }
            if Self::smaller(tree, depth, v, self.heap[j] as usize) {
                break;
            }
            self.heap[k] = self.heap[j];
            k = j;
            j <<= 1;
        }
        self.heap[k] = v as i32;
    }

    fn gen_bitlen(&mut self, which: Tree, max_code: i32) {
        let (stree, extra, base, max_length): (Option<&[Ct]>, &[u32], usize, usize) = match which {
            Tree::L => (
                Some(&TABLES.ltree[..]),
                &EXTRA_LBITS[..],
                LITERALS + 1,
                MAX_BITS,
            ),
            Tree::D => (Some(&TABLES.dtree[..]), &EXTRA_DBITS[..], 0, MAX_BITS),
            Tree::Bl => (None, &EXTRA_BLBITS[..], 0, MAX_BL_BITS),
        };
        self.bl_count = [0; MAX_BITS + 1];
        let heap = self.heap;
        let heap_max = self.heap_max as usize;
        let mut opt_len = self.s.opt_len;
        let mut static_len = self.s.static_len;
        let mut bl_count = self.bl_count;
        let tree = self.tree(which);
        let mut overflow = 0;
        let top = heap[heap_max] as usize;
        tree[top].dl = 0;
        let mut h = heap_max + 1;
        while h < HEAP_SIZE {
            let n = heap[h] as usize;
            let mut bits = tree[tree[n].dl as usize].dl as usize + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            tree[n].dl = bits as u16;
            h += 1;
            if n as i32 > max_code {
                continue;
            }
            bl_count[bits] += 1;
            let xbits = if n >= base { extra[n - base] } else { 0 };
            let f = u64::from(tree[n].fc);
            opt_len = opt_len.wrapping_add(f * (bits as u64 + u64::from(xbits)));
            if let Some(stree) = stree {
                static_len =
                    static_len.wrapping_add(f * (u64::from(stree[n].dl) + u64::from(xbits)));
            }
        }
        if overflow != 0 {
            loop {
                let mut bits = max_length - 1;
                while bl_count[bits] == 0 {
                    bits -= 1;
                }
                bl_count[bits] -= 1;
                bl_count[bits + 1] += 2;
                bl_count[max_length] -= 1;
                overflow -= 2;
                if overflow <= 0 {
                    break;
                }
            }
            let mut bits = max_length;
            while bits != 0 {
                let mut n = u32::from(bl_count[bits]);
                while n != 0 {
                    h -= 1;
                    let m = heap[h] as usize;
                    if m as i32 > max_code {
                        continue;
                    }
                    if u32::from(tree[m].dl) != bits as u32 {
                        opt_len = opt_len.wrapping_add(
                            (bits as u64)
                                .wrapping_sub(u64::from(tree[m].dl))
                                .wrapping_mul(u64::from(tree[m].fc)),
                        );
                        tree[m].dl = bits as u16;
                    }
                    n -= 1;
                }
                bits -= 1;
            }
        }
        self.bl_count = bl_count;
        self.s.opt_len = opt_len;
        self.s.static_len = static_len;
    }

    fn gen_codes(tree: &mut [Ct], max_code: i32, bl_count: &[u16; MAX_BITS + 1]) {
        let mut next_code = [0u16; MAX_BITS + 1];
        let mut code = 0u32;
        for bits in 1..=MAX_BITS {
            code = (code + u32::from(bl_count[bits - 1])) << 1;
            next_code[bits] = code as u16;
        }
        for n in 0..=max_code.max(-1) {
            let n = n as usize;
            let len = tree[n].dl as usize;
            if len == 0 || len > MAX_BITS {
                continue;
            }
            tree[n].fc = bi_reverse(u32::from(next_code[len]), len as i32) as u16;
            next_code[len] = next_code[len].wrapping_add(1);
        }
    }

    fn build_tree(&mut self, which: Tree) {
        let (stree, elems): (Option<&[Ct]>, usize) = match which {
            Tree::L => (Some(&TABLES.ltree[..]), L_CODES),
            Tree::D => (Some(&TABLES.dtree[..]), D_CODES),
            Tree::Bl => (None, BL_CODES),
        };
        let mut max_code: i32 = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE as i32;
        for n in 0..elems {
            if self.tree(which)[n].fc != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len as usize] = n as i32;
                max_code = n as i32;
                self.depth[n] = 0;
            } else {
                self.tree(which)[n].dl = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code
            } else {
                0
            } as usize;
            self.heap_len += 1;
            self.heap[self.heap_len as usize] = node as i32;
            self.tree(which)[node].fc = 1;
            self.depth[node] = 0;
            self.s.opt_len = self.s.opt_len.wrapping_sub(1);
            if let Some(stree) = stree {
                self.s.static_len = self.s.static_len.wrapping_sub(u64::from(stree[node].dl));
            }
        }
        match which {
            Tree::L => self.l_max_code = max_code,
            Tree::D => self.d_max_code = max_code,
            Tree::Bl => self.bl_max_code = max_code,
        }
        let mut n = self.heap_len / 2;
        while n >= 1 {
            self.pqdownheap(which, n as usize);
            n -= 1;
        }
        let mut node = elems;
        loop {
            // pqremove
            let n = self.heap[1] as usize;
            self.heap[1] = self.heap[self.heap_len as usize];
            self.heap_len -= 1;
            self.pqdownheap(which, 1);
            let m = self.heap[1] as usize;
            self.heap_max -= 1;
            self.heap[self.heap_max as usize] = n as i32;
            self.heap_max -= 1;
            self.heap[self.heap_max as usize] = m as i32;
            let depth = self.depth[n].max(self.depth[m]).wrapping_add(1);
            let tree = self.tree(which);
            tree[node].fc = tree[n].fc.wrapping_add(tree[m].fc);
            tree[n].dl = node as u16;
            tree[m].dl = node as u16;
            self.depth[node] = depth;
            self.heap[1] = node as i32;
            node += 1;
            self.pqdownheap(which, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max as usize] = self.heap[1];
        self.gen_bitlen(which, max_code);
        let bl_count = self.bl_count;
        Self::gen_codes(self.tree(which), max_code, &bl_count);
    }

    fn scan_tree(&mut self, which: Tree, max_code: i32) {
        let tree = self.tree(which);
        let mut prevlen: i32 = -1;
        let mut nextlen = i32::from(tree[0].dl);
        let mut count = 0;
        let mut max_count = 7;
        let mut min_count = 4;
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        tree[(max_code + 1) as usize].dl = 0xffff;
        let mut bl = self.bltree;
        let tree = self.tree(which);
        for n in 0..=max_code as usize {
            let curlen = nextlen;
            nextlen = i32::from(tree[n + 1].dl);
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                bl[curlen as usize].fc = bl[curlen as usize].fc.wrapping_add(count as u16);
            } else if curlen != 0 {
                if curlen != prevlen {
                    bl[curlen as usize].fc = bl[curlen as usize].fc.wrapping_add(1);
                }
                bl[REP_3_6].fc = bl[REP_3_6].fc.wrapping_add(1);
            } else if count <= 10 {
                bl[REPZ_3_10].fc = bl[REPZ_3_10].fc.wrapping_add(1);
            } else {
                bl[REPZ_11_138].fc = bl[REPZ_11_138].fc.wrapping_add(1);
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
        self.bltree = bl;
    }

    fn send_tree(&mut self, which: Tree, max_code: i32) {
        let lens: Vec<u16> = self.tree(which)[..=(max_code + 1) as usize]
            .iter()
            .map(|ct| ct.dl)
            .collect();
        let mut prevlen: i32 = -1;
        let mut nextlen = i32::from(lens[0]);
        let mut count = 0;
        let mut max_count = 7;
        let mut min_count = 4;
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        for n in 0..=max_code as usize {
            let curlen = nextlen;
            nextlen = i32::from(lens[n + 1]);
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                loop {
                    self.send_code(curlen as usize, Tree::Bl);
                    count -= 1;
                    if count == 0 {
                        break;
                    }
                }
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.send_code(curlen as usize, Tree::Bl);
                    count -= 1;
                }
                self.send_code(REP_3_6, Tree::Bl);
                self.send_bits((count - 3) as u32, 2);
            } else if count <= 10 {
                self.send_code(REPZ_3_10, Tree::Bl);
                self.send_bits((count - 3) as u32, 3);
            } else {
                self.send_code(REPZ_11_138, Tree::Bl);
                self.send_bits((count - 11) as u32, 7);
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    fn build_bl_tree(&mut self) -> usize {
        self.scan_tree(Tree::L, self.l_max_code);
        self.scan_tree(Tree::D, self.d_max_code);
        self.build_tree(Tree::Bl);
        let mut max_blindex = BL_CODES - 1;
        while max_blindex >= 3 {
            if self.bltree[BL_ORDER[max_blindex]].dl != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.s.opt_len = self
            .s
            .opt_len
            .wrapping_add(3 * (max_blindex as u64 + 1) + 5 + 5 + 4);
        max_blindex
    }

    fn send_all_trees(&mut self, lcodes: i32, dcodes: i32, blcodes: usize) {
        self.send_bits((lcodes - 257) as u32, 5);
        self.send_bits((dcodes - 1) as u32, 5);
        self.send_bits(blcodes as u32 - 4, 4);
        for &order in BL_ORDER.iter().take(blcodes) {
            let len = u32::from(self.bltree[order].dl);
            self.send_bits(len, 3);
        }
        self.send_tree(Tree::L, lcodes - 1);
        self.send_tree(Tree::D, dcodes - 1);
    }

    fn tr_stored_block(&mut self, buf: Option<usize>, stored_len: u64, last: u32) {
        self.send_bits((STORED_BLOCK << 1) + last, 3);
        self.bi_windup();
        self.put_short(stored_len as u16);
        self.put_short(!(stored_len as u16));
        if stored_len != 0 {
            if let Some(from) = buf {
                let at = self.s.pending as usize;
                let len = stored_len as usize;
                if from + len <= self.window.len() && at + len <= self.pbuf.len() {
                    self.pbuf[at..at + len].copy_from_slice(&self.window[from..from + len]);
                }
            }
        }
        self.s.pending += stored_len as u32;
    }

    fn tr_align(&mut self) {
        self.send_bits(STATIC_TREES << 1, 3);
        let ct = TABLES.ltree[END_BLOCK];
        self.send_bits(u32::from(ct.fc), i32::from(ct.dl));
        self.bi_flush();
    }

    fn compress_block(&mut self, dynamic: bool) {
        let mut sx = 0u32;
        let base = self.s.lit_bufsize as usize;
        if self.s.sym_next != 0 {
            loop {
                let at = base + sx as usize;
                let b0 = u32::from(self.pbuf.get(at).copied().unwrap_or(0));
                let b1 = u32::from(self.pbuf.get(at + 1).copied().unwrap_or(0));
                let lc = self.pbuf.get(at + 2).copied().unwrap_or(0) as usize;
                sx += 3;
                let mut dist = b0 + (b1 << 8);
                if dist == 0 {
                    self.send_lcode(lc, dynamic);
                } else {
                    let code = TABLES.length_code[lc] as usize;
                    self.send_lcode(code + LITERALS + 1, dynamic);
                    let extra = EXTRA_LBITS[code];
                    if extra != 0 {
                        let lc = lc as u32 - TABLES.base_length[code];
                        self.send_bits(lc, extra as i32);
                    }
                    dist -= 1;
                    let code = d_code(dist);
                    if dynamic {
                        self.send_code(code, Tree::D);
                    } else {
                        let ct = TABLES.dtree[code];
                        self.send_bits(u32::from(ct.fc), i32::from(ct.dl));
                    }
                    let extra = EXTRA_DBITS[code];
                    if extra != 0 {
                        dist -= TABLES.base_dist[code];
                        self.send_bits(dist, extra as i32);
                    }
                }
                if sx >= self.s.sym_next {
                    break;
                }
            }
        }
        self.send_lcode(END_BLOCK, dynamic);
    }

    fn send_lcode(&mut self, c: usize, dynamic: bool) {
        if dynamic {
            self.send_code(c, Tree::L);
        } else {
            let ct = TABLES.ltree[c];
            self.send_bits(u32::from(ct.fc), i32::from(ct.dl));
        }
    }

    fn detect_data_type(&self) -> i32 {
        let mut block_mask: u64 = 0xf3ff_c07f;
        for n in 0..=31 {
            if block_mask & 1 != 0 && self.ltree[n].fc != 0 {
                return Z_BINARY;
            }
            block_mask >>= 1;
        }
        if self.ltree[9].fc != 0 || self.ltree[10].fc != 0 || self.ltree[13].fc != 0 {
            return Z_TEXT;
        }
        for n in 32..LITERALS {
            if self.ltree[n].fc != 0 {
                return Z_TEXT;
            }
        }
        Z_BINARY
    }

    fn tr_flush_block(&mut self, z: &mut Strm, buf: Option<usize>, stored_len: u64, last: u32) {
        let mut max_blindex = 0;
        let mut opt_lenb;
        let static_lenb;
        if self.s.level > 0 {
            if z.data_type == Z_UNKNOWN {
                z.data_type = self.detect_data_type();
            }
            self.build_tree(Tree::L);
            self.build_tree(Tree::D);
            max_blindex = self.build_bl_tree();
            opt_lenb = self.s.opt_len.wrapping_add(3 + 7) >> 3;
            static_lenb = self.s.static_len.wrapping_add(3 + 7) >> 3;
            if static_lenb <= opt_lenb || self.s.strategy == Z_FIXED {
                opt_lenb = static_lenb;
            }
        } else {
            opt_lenb = stored_len + 5;
            static_lenb = opt_lenb;
        }
        if stored_len + 4 <= opt_lenb && buf.is_some() {
            self.tr_stored_block(buf, stored_len, last);
        } else if static_lenb == opt_lenb {
            self.send_bits((STATIC_TREES << 1) + last, 3);
            self.compress_block(false);
        } else {
            self.send_bits((DYN_TREES << 1) + last, 3);
            self.send_all_trees(self.l_max_code + 1, self.d_max_code + 1, max_blindex + 1);
            self.compress_block(true);
        }
        self.init_block();
        if last != 0 {
            self.bi_windup();
        }
    }
}

/// `deflateInit2`: a new stream's state bytes, or zlib's error.
pub fn deflate_init(
    level: i32,
    method: i32,
    window_bits: i32,
    mem_level: i32,
    strategy: i32,
) -> Result<Vec<u8>, i32> {
    let mut wrap = 1;
    let mut level = level;
    let mut window_bits = window_bits;
    if level == -1 {
        level = 6;
    }
    if window_bits < 0 {
        wrap = 0;
        if window_bits < -15 {
            return Err(Z_STREAM_ERROR);
        }
        window_bits = -window_bits;
    } else if window_bits > 15 {
        wrap = 2;
        window_bits -= 16;
    }
    if !(1..=9).contains(&mem_level)
        || method != Z_DEFLATED
        || !(8..=15).contains(&window_bits)
        || !(0..=9).contains(&level)
        || !(0..=Z_FIXED).contains(&strategy)
        || (window_bits == 8 && wrap != 1)
    {
        return Err(Z_STREAM_ERROR);
    }
    if window_bits == 8 {
        window_bits = 9;
    }
    let w_bits = window_bits as u32;
    let mem_level = mem_level as u32;
    let mut bytes = vec![0u8; deflate_size(w_bits, mem_level)];
    bytes[..8].copy_from_slice(DEFLATE_MAGIC);
    let w_size = 1u32 << w_bits;
    let hash_bits = mem_level + 7;
    let lit_bufsize = 1u32 << (mem_level + 6);
    let s = DeflateScalars {
        status: INIT_STATE,
        wrap,
        w_bits,
        w_size,
        w_mask: w_size - 1,
        hash_bits,
        hash_size: 1 << hash_bits,
        hash_mask: (1 << hash_bits) - 1,
        hash_shift: hash_bits.div_ceil(MIN_MATCH),
        lit_bufsize,
        pending_buf_size: u64::from(lit_bufsize) * 4,
        sym_end: (lit_bufsize - 1) * 3,
        level,
        strategy,
        mem_level,
        ..DeflateScalars::default()
    };
    s.write(&mut bytes[8..]);
    let Some(mut d) = Deflate::open(&mut bytes) else {
        return Err(Z_STREAM_ERROR);
    };
    let mut z = Strm::new(&[], 0);
    d.reset(&mut z);
    d.store_strm(&z);
    d.save();
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// The inflate state: inflate.h's `inflate_state`.

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
enum Mode {
    Head = 0,
    Flags,
    Time,
    Os,
    Exlen,
    Extra,
    Name,
    Comment,
    Hcrc,
    Dictid,
    Dict,
    Type,
    Typedo,
    Stored,
    Copy_,
    Copy,
    Table,
    Lenlens,
    Codelens,
    Len_,
    Len,
    Lenext,
    Dist,
    Distext,
    Match,
    Lit,
    Check,
    Length,
    Done,
    Bad,
    Mem,
    Sync,
}

impl Mode {
    fn from(n: u8) -> Option<Mode> {
        const ALL: [Mode; 32] = [
            Mode::Head,
            Mode::Flags,
            Mode::Time,
            Mode::Os,
            Mode::Exlen,
            Mode::Extra,
            Mode::Name,
            Mode::Comment,
            Mode::Hcrc,
            Mode::Dictid,
            Mode::Dict,
            Mode::Type,
            Mode::Typedo,
            Mode::Stored,
            Mode::Copy_,
            Mode::Copy,
            Mode::Table,
            Mode::Lenlens,
            Mode::Codelens,
            Mode::Len_,
            Mode::Len,
            Mode::Lenext,
            Mode::Dist,
            Mode::Distext,
            Mode::Match,
            Mode::Lit,
            Mode::Check,
            Mode::Length,
            Mode::Done,
            Mode::Bad,
            Mode::Mem,
            Mode::Sync,
        ];
        ALL.get(n as usize).copied()
    }
}

/// `code`: a decoding table entry.
#[derive(Clone, Copy, Default)]
struct Code {
    op: u8,
    bits: u8,
    val: u16,
}

const fn c(op: u8, bits: u8, val: u16) -> Code {
    Code { op, bits, val }
}

// inffixed.h: the fixed code's tables, as zlib ships them.
#[rustfmt::skip]
const LENFIX: [Code; 512] = [
    c(96, 7, 0), c(0, 8, 80), c(0, 8, 16), c(20, 8, 115), c(18, 7, 31), c(0, 8, 112), c(0, 8, 48),
    c(0, 9, 192), c(16, 7, 10), c(0, 8, 96), c(0, 8, 32), c(0, 9, 160), c(0, 8, 0), c(0, 8, 128),
    c(0, 8, 64), c(0, 9, 224), c(16, 7, 6), c(0, 8, 88), c(0, 8, 24), c(0, 9, 144), c(19, 7, 59),
    c(0, 8, 120), c(0, 8, 56), c(0, 9, 208), c(17, 7, 17), c(0, 8, 104), c(0, 8, 40), c(0, 9, 176),
    c(0, 8, 8), c(0, 8, 136), c(0, 8, 72), c(0, 9, 240), c(16, 7, 4), c(0, 8, 84), c(0, 8, 20),
    c(21, 8, 227), c(19, 7, 43), c(0, 8, 116), c(0, 8, 52), c(0, 9, 200), c(17, 7, 13),
    c(0, 8, 100), c(0, 8, 36), c(0, 9, 168), c(0, 8, 4), c(0, 8, 132), c(0, 8, 68), c(0, 9, 232),
    c(16, 7, 8), c(0, 8, 92), c(0, 8, 28), c(0, 9, 152), c(20, 7, 83), c(0, 8, 124), c(0, 8, 60),
    c(0, 9, 216), c(18, 7, 23), c(0, 8, 108), c(0, 8, 44), c(0, 9, 184), c(0, 8, 12), c(0, 8, 140),
    c(0, 8, 76), c(0, 9, 248), c(16, 7, 3), c(0, 8, 82), c(0, 8, 18), c(21, 8, 163), c(19, 7, 35),
    c(0, 8, 114), c(0, 8, 50), c(0, 9, 196), c(17, 7, 11), c(0, 8, 98), c(0, 8, 34), c(0, 9, 164),
    c(0, 8, 2), c(0, 8, 130), c(0, 8, 66), c(0, 9, 228), c(16, 7, 7), c(0, 8, 90), c(0, 8, 26),
    c(0, 9, 148), c(20, 7, 67), c(0, 8, 122), c(0, 8, 58), c(0, 9, 212), c(18, 7, 19),
    c(0, 8, 106), c(0, 8, 42), c(0, 9, 180), c(0, 8, 10), c(0, 8, 138), c(0, 8, 74), c(0, 9, 244),
    c(16, 7, 5), c(0, 8, 86), c(0, 8, 22), c(64, 8, 0), c(19, 7, 51), c(0, 8, 118), c(0, 8, 54),
    c(0, 9, 204), c(17, 7, 15), c(0, 8, 102), c(0, 8, 38), c(0, 9, 172), c(0, 8, 6), c(0, 8, 134),
    c(0, 8, 70), c(0, 9, 236), c(16, 7, 9), c(0, 8, 94), c(0, 8, 30), c(0, 9, 156), c(20, 7, 99),
    c(0, 8, 126), c(0, 8, 62), c(0, 9, 220), c(18, 7, 27), c(0, 8, 110), c(0, 8, 46), c(0, 9, 188),
    c(0, 8, 14), c(0, 8, 142), c(0, 8, 78), c(0, 9, 252), c(96, 7, 0), c(0, 8, 81), c(0, 8, 17),
    c(21, 8, 131), c(18, 7, 31), c(0, 8, 113), c(0, 8, 49), c(0, 9, 194), c(16, 7, 10),
    c(0, 8, 97), c(0, 8, 33), c(0, 9, 162), c(0, 8, 1), c(0, 8, 129), c(0, 8, 65), c(0, 9, 226),
    c(16, 7, 6), c(0, 8, 89), c(0, 8, 25), c(0, 9, 146), c(19, 7, 59), c(0, 8, 121), c(0, 8, 57),
    c(0, 9, 210), c(17, 7, 17), c(0, 8, 105), c(0, 8, 41), c(0, 9, 178), c(0, 8, 9), c(0, 8, 137),
    c(0, 8, 73), c(0, 9, 242), c(16, 7, 4), c(0, 8, 85), c(0, 8, 21), c(16, 8, 258), c(19, 7, 43),
    c(0, 8, 117), c(0, 8, 53), c(0, 9, 202), c(17, 7, 13), c(0, 8, 101), c(0, 8, 37), c(0, 9, 170),
    c(0, 8, 5), c(0, 8, 133), c(0, 8, 69), c(0, 9, 234), c(16, 7, 8), c(0, 8, 93), c(0, 8, 29),
    c(0, 9, 154), c(20, 7, 83), c(0, 8, 125), c(0, 8, 61), c(0, 9, 218), c(18, 7, 23),
    c(0, 8, 109), c(0, 8, 45), c(0, 9, 186), c(0, 8, 13), c(0, 8, 141), c(0, 8, 77), c(0, 9, 250),
    c(16, 7, 3), c(0, 8, 83), c(0, 8, 19), c(21, 8, 195), c(19, 7, 35), c(0, 8, 115), c(0, 8, 51),
    c(0, 9, 198), c(17, 7, 11), c(0, 8, 99), c(0, 8, 35), c(0, 9, 166), c(0, 8, 3), c(0, 8, 131),
    c(0, 8, 67), c(0, 9, 230), c(16, 7, 7), c(0, 8, 91), c(0, 8, 27), c(0, 9, 150), c(20, 7, 67),
    c(0, 8, 123), c(0, 8, 59), c(0, 9, 214), c(18, 7, 19), c(0, 8, 107), c(0, 8, 43), c(0, 9, 182),
    c(0, 8, 11), c(0, 8, 139), c(0, 8, 75), c(0, 9, 246), c(16, 7, 5), c(0, 8, 87), c(0, 8, 23),
    c(64, 8, 0), c(19, 7, 51), c(0, 8, 119), c(0, 8, 55), c(0, 9, 206), c(17, 7, 15), c(0, 8, 103),
    c(0, 8, 39), c(0, 9, 174), c(0, 8, 7), c(0, 8, 135), c(0, 8, 71), c(0, 9, 238), c(16, 7, 9),
    c(0, 8, 95), c(0, 8, 31), c(0, 9, 158), c(20, 7, 99), c(0, 8, 127), c(0, 8, 63), c(0, 9, 222),
    c(18, 7, 27), c(0, 8, 111), c(0, 8, 47), c(0, 9, 190), c(0, 8, 15), c(0, 8, 143), c(0, 8, 79),
    c(0, 9, 254), c(96, 7, 0), c(0, 8, 80), c(0, 8, 16), c(20, 8, 115), c(18, 7, 31), c(0, 8, 112),
    c(0, 8, 48), c(0, 9, 193), c(16, 7, 10), c(0, 8, 96), c(0, 8, 32), c(0, 9, 161), c(0, 8, 0),
    c(0, 8, 128), c(0, 8, 64), c(0, 9, 225), c(16, 7, 6), c(0, 8, 88), c(0, 8, 24), c(0, 9, 145),
    c(19, 7, 59), c(0, 8, 120), c(0, 8, 56), c(0, 9, 209), c(17, 7, 17), c(0, 8, 104), c(0, 8, 40),
    c(0, 9, 177), c(0, 8, 8), c(0, 8, 136), c(0, 8, 72), c(0, 9, 241), c(16, 7, 4), c(0, 8, 84),
    c(0, 8, 20), c(21, 8, 227), c(19, 7, 43), c(0, 8, 116), c(0, 8, 52), c(0, 9, 201),
    c(17, 7, 13), c(0, 8, 100), c(0, 8, 36), c(0, 9, 169), c(0, 8, 4), c(0, 8, 132), c(0, 8, 68),
    c(0, 9, 233), c(16, 7, 8), c(0, 8, 92), c(0, 8, 28), c(0, 9, 153), c(20, 7, 83), c(0, 8, 124),
    c(0, 8, 60), c(0, 9, 217), c(18, 7, 23), c(0, 8, 108), c(0, 8, 44), c(0, 9, 185), c(0, 8, 12),
    c(0, 8, 140), c(0, 8, 76), c(0, 9, 249), c(16, 7, 3), c(0, 8, 82), c(0, 8, 18), c(21, 8, 163),
    c(19, 7, 35), c(0, 8, 114), c(0, 8, 50), c(0, 9, 197), c(17, 7, 11), c(0, 8, 98), c(0, 8, 34),
    c(0, 9, 165), c(0, 8, 2), c(0, 8, 130), c(0, 8, 66), c(0, 9, 229), c(16, 7, 7), c(0, 8, 90),
    c(0, 8, 26), c(0, 9, 149), c(20, 7, 67), c(0, 8, 122), c(0, 8, 58), c(0, 9, 213), c(18, 7, 19),
    c(0, 8, 106), c(0, 8, 42), c(0, 9, 181), c(0, 8, 10), c(0, 8, 138), c(0, 8, 74), c(0, 9, 245),
    c(16, 7, 5), c(0, 8, 86), c(0, 8, 22), c(64, 8, 0), c(19, 7, 51), c(0, 8, 118), c(0, 8, 54),
    c(0, 9, 205), c(17, 7, 15), c(0, 8, 102), c(0, 8, 38), c(0, 9, 173), c(0, 8, 6), c(0, 8, 134),
    c(0, 8, 70), c(0, 9, 237), c(16, 7, 9), c(0, 8, 94), c(0, 8, 30), c(0, 9, 157), c(20, 7, 99),
    c(0, 8, 126), c(0, 8, 62), c(0, 9, 221), c(18, 7, 27), c(0, 8, 110), c(0, 8, 46), c(0, 9, 189),
    c(0, 8, 14), c(0, 8, 142), c(0, 8, 78), c(0, 9, 253), c(96, 7, 0), c(0, 8, 81), c(0, 8, 17),
    c(21, 8, 131), c(18, 7, 31), c(0, 8, 113), c(0, 8, 49), c(0, 9, 195), c(16, 7, 10),
    c(0, 8, 97), c(0, 8, 33), c(0, 9, 163), c(0, 8, 1), c(0, 8, 129), c(0, 8, 65), c(0, 9, 227),
    c(16, 7, 6), c(0, 8, 89), c(0, 8, 25), c(0, 9, 147), c(19, 7, 59), c(0, 8, 121), c(0, 8, 57),
    c(0, 9, 211), c(17, 7, 17), c(0, 8, 105), c(0, 8, 41), c(0, 9, 179), c(0, 8, 9), c(0, 8, 137),
    c(0, 8, 73), c(0, 9, 243), c(16, 7, 4), c(0, 8, 85), c(0, 8, 21), c(16, 8, 258), c(19, 7, 43),
    c(0, 8, 117), c(0, 8, 53), c(0, 9, 203), c(17, 7, 13), c(0, 8, 101), c(0, 8, 37), c(0, 9, 171),
    c(0, 8, 5), c(0, 8, 133), c(0, 8, 69), c(0, 9, 235), c(16, 7, 8), c(0, 8, 93), c(0, 8, 29),
    c(0, 9, 155), c(20, 7, 83), c(0, 8, 125), c(0, 8, 61), c(0, 9, 219), c(18, 7, 23),
    c(0, 8, 109), c(0, 8, 45), c(0, 9, 187), c(0, 8, 13), c(0, 8, 141), c(0, 8, 77), c(0, 9, 251),
    c(16, 7, 3), c(0, 8, 83), c(0, 8, 19), c(21, 8, 195), c(19, 7, 35), c(0, 8, 115), c(0, 8, 51),
    c(0, 9, 199), c(17, 7, 11), c(0, 8, 99), c(0, 8, 35), c(0, 9, 167), c(0, 8, 3), c(0, 8, 131),
    c(0, 8, 67), c(0, 9, 231), c(16, 7, 7), c(0, 8, 91), c(0, 8, 27), c(0, 9, 151), c(20, 7, 67),
    c(0, 8, 123), c(0, 8, 59), c(0, 9, 215), c(18, 7, 19), c(0, 8, 107), c(0, 8, 43), c(0, 9, 183),
    c(0, 8, 11), c(0, 8, 139), c(0, 8, 75), c(0, 9, 247), c(16, 7, 5), c(0, 8, 87), c(0, 8, 23),
    c(64, 8, 0), c(19, 7, 51), c(0, 8, 119), c(0, 8, 55), c(0, 9, 207), c(17, 7, 15), c(0, 8, 103),
    c(0, 8, 39), c(0, 9, 175), c(0, 8, 7), c(0, 8, 135), c(0, 8, 71), c(0, 9, 239), c(16, 7, 9),
    c(0, 8, 95), c(0, 8, 31), c(0, 9, 159), c(20, 7, 99), c(0, 8, 127), c(0, 8, 63), c(0, 9, 223),
    c(18, 7, 27), c(0, 8, 111), c(0, 8, 47), c(0, 9, 191), c(0, 8, 15), c(0, 8, 143), c(0, 8, 79),
    c(0, 9, 255),
];
#[rustfmt::skip]
const DISTFIX: [Code; 32] = [
    c(16, 5, 1), c(23, 5, 257), c(19, 5, 17), c(27, 5, 4097), c(17, 5, 5), c(25, 5, 1025),
    c(21, 5, 65), c(29, 5, 16385), c(16, 5, 3), c(24, 5, 513), c(20, 5, 33), c(28, 5, 8193),
    c(18, 5, 9), c(26, 5, 2049), c(22, 5, 129), c(64, 5, 0), c(16, 5, 2), c(23, 5, 385),
    c(19, 5, 25), c(27, 5, 6145), c(17, 5, 7), c(25, 5, 1537), c(21, 5, 97), c(29, 5, 24577),
    c(16, 5, 4), c(24, 5, 769), c(20, 5, 49), c(28, 5, 12289), c(18, 5, 13), c(26, 5, 3073),
    c(22, 5, 193), c(64, 5, 0),
];

const ENOUGH_LENS: usize = 852;
const ENOUGH_DISTS: usize = 592;
const ENOUGH: usize = ENOUGH_LENS + ENOUGH_DISTS;

/// Where `lencode` and `distcode` point: an offset into `codes`, or one of
/// the fixed tables.
const LENFIX_AT: u32 = 0x1_0000;
const DISTFIX_AT: u32 = 0x2_0000;

scalars!(InflateScalars {
    mode: u8,
    last: i32,
    wrap: i32,
    havedict: i32,
    flags: i32,
    dmax: u32,
    check: u64,
    total: u64,
    wbits: u32,
    wsize: u32,
    whave: u32,
    wnext: u32,
    hold: u64,
    bits: u32,
    length: u32,
    offset: u32,
    extra: u32,
    lencode: u32,
    distcode: u32,
    lenbits: u32,
    distbits: u32,
    ncode: u32,
    nlen: u32,
    ndist: u32,
    have: u32,
    next: u32,
    sane: i32,
    back: i32,
    was: u32,
    total_in: u64,
    total_out: u64,
    adler: u64,
    data_type: i32,
    msg: u8,
});

const INFLATE_MAGIC: &[u8; 8] = b"SpZlInf1";
const INFLATE_WINDOW: usize = 1 << 15;
const INFLATE_HEADER: usize = 8 + InflateScalars::COUNT * 8 + 320 * 2 + ENOUGH * 4;
const INFLATE_SIZE: usize = INFLATE_HEADER + INFLATE_WINDOW;

struct Inflate<'b> {
    s: InflateScalars,
    lens: [u16; 320],
    work: [u16; 288],
    codes: [Code; ENOUGH],
    header: &'b mut [u8],
    window: &'b mut [u8],
}

#[derive(Clone, Copy, PartialEq)]
enum CodeType {
    Codes,
    Lens,
    Dists,
}

/// `inflate_table`: build a decoding table for `lens` at `table[*next..]`.
fn inflate_table(
    kind: CodeType,
    lens: &[u16],
    table: &mut [Code],
    next_at: &mut usize,
    bits: &mut u32,
    work: &mut [u16; 288],
) -> i32 {
    const LBASE: [u16; 31] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258, 0, 0,
    ];
    const LEXT: [u16; 31] = [
        16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 18, 18, 18, 18, 19, 19, 19, 19, 20, 20, 20,
        20, 21, 21, 21, 21, 16, 203, 77,
    ];
    const DBASE: [u16; 32] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 0, 0,
    ];
    const DEXT: [u16; 32] = [
        16, 16, 16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 24, 25, 25, 26,
        26, 27, 27, 28, 28, 29, 29, 64, 64,
    ];
    const MAXBITS: usize = 15;
    let codes = lens.len();
    let mut count = [0u16; MAXBITS + 1];
    let mut offs = [0u16; MAXBITS + 1];
    for &l in lens {
        if let Some(c) = count.get_mut(l as usize) {
            *c += 1;
        } else {
            return -1;
        }
    }
    let mut root = *bits;
    let mut max = MAXBITS;
    while max >= 1 {
        if count[max] != 0 {
            break;
        }
        max -= 1;
    }
    if root > max as u32 {
        root = max as u32;
    }
    if max == 0 {
        let here = Code {
            op: 64,
            bits: 1,
            val: 0,
        };
        for _ in 0..2 {
            if let Some(slot) = table.get_mut(*next_at) {
                *slot = here;
            }
            *next_at += 1;
        }
        *bits = 1;
        return 0;
    }
    let mut min = 1;
    while min < max {
        if count[min] != 0 {
            break;
        }
        min += 1;
    }
    if root < min as u32 {
        root = min as u32;
    }
    let mut left: i32 = 1;
    for &n in &count[1..=MAXBITS] {
        left <<= 1;
        left -= i32::from(n);
        if left < 0 {
            return -1;
        }
    }
    if left > 0 && (kind == CodeType::Codes || max != 1) {
        return -1;
    }
    offs[1] = 0;
    for len in 1..MAXBITS {
        offs[len + 1] = offs[len] + count[len];
    }
    for (sym, &l) in lens.iter().enumerate().take(codes) {
        if l != 0 {
            let o = &mut offs[l as usize];
            if let Some(w) = work.get_mut(*o as usize) {
                *w = sym as u16;
            }
            *o += 1;
        }
    }
    let (base, extra, matchv): (&[u16], &[u16], u32) = match kind {
        CodeType::Codes => (&[], &[], 20),
        CodeType::Lens => (&LBASE, &LEXT, 257),
        CodeType::Dists => (&DBASE, &DEXT, 0),
    };
    let mut huff: u32 = 0;
    let mut sym: usize = 0;
    let mut len = min as u32;
    let mut next = *next_at;
    let mut curr = root;
    let mut drop: u32 = 0;
    let mut low: u32 = u32::MAX;
    let mut used: u32 = 1 << root;
    let mask = used - 1;
    if (kind == CodeType::Lens && used as usize > ENOUGH_LENS)
        || (kind == CodeType::Dists && used as usize > ENOUGH_DISTS)
    {
        return 1;
    }
    loop {
        let w = u32::from(work[sym.min(287)]);
        let here = if w + 1 < matchv {
            Code {
                op: 0,
                bits: (len - drop) as u8,
                val: w as u16,
            }
        } else if w >= matchv {
            let i = (w - matchv) as usize;
            let (op, val) = if kind == CodeType::Codes {
                (0, 0)
            } else {
                (
                    extra.get(i).copied().unwrap_or(64) as u8,
                    base.get(i).copied().unwrap_or(0),
                )
            };
            Code {
                op,
                bits: (len - drop) as u8,
                val,
            }
        } else {
            Code {
                op: 32 + 64,
                bits: (len - drop) as u8,
                val: 0,
            }
        };
        let incr = 1u32 << (len - drop);
        let mut fill = 1u32 << curr;
        min = fill as usize;
        loop {
            fill -= incr;
            if let Some(slot) = table.get_mut(next + ((huff >> drop) + fill) as usize) {
                *slot = here;
            }
            if fill == 0 {
                break;
            }
        }
        let mut incr = 1u32 << (len - 1);
        while huff & incr != 0 {
            incr >>= 1;
        }
        if incr != 0 {
            huff &= incr - 1;
            huff += incr;
        } else {
            huff = 0;
        }
        sym += 1;
        count[len as usize] -= 1;
        if count[len as usize] == 0 {
            if len as usize == max {
                break;
            }
            len = u32::from(lens[work[sym.min(287)] as usize]);
        }
        if len > root && (huff & mask) != low {
            if drop == 0 {
                drop = root;
            }
            next += min;
            curr = len - drop;
            let mut left: i32 = 1 << curr;
            while ((curr + drop) as usize) < max {
                left -= i32::from(count[(curr + drop) as usize]);
                if left <= 0 {
                    break;
                }
                curr += 1;
                left <<= 1;
            }
            used += 1 << curr;
            if (kind == CodeType::Lens && used as usize > ENOUGH_LENS)
                || (kind == CodeType::Dists && used as usize > ENOUGH_DISTS)
            {
                return 1;
            }
            low = huff & mask;
            if let Some(slot) = table.get_mut(*next_at + low as usize) {
                *slot = Code {
                    op: curr as u8,
                    bits: root as u8,
                    val: (next - *next_at) as u16,
                };
            }
        }
    }
    if huff != 0 {
        if let Some(slot) = table.get_mut(next + huff as usize) {
            *slot = Code {
                op: 64,
                bits: (len - drop) as u8,
                val: 0,
            };
        }
    }
    *next_at += used as usize;
    *bits = root;
    0
}

impl<'b> Inflate<'b> {
    fn open(bytes: &'b mut [u8]) -> Option<Inflate<'b>> {
        if bytes.len() != INFLATE_SIZE || &bytes[..8] != INFLATE_MAGIC {
            return None;
        }
        let s = InflateScalars::read(&bytes[8..]);
        let table_ok = |at: u32| at == LENFIX_AT || at == DISTFIX_AT || (at as usize) < ENOUGH;
        let sane = Mode::from(s.mode).is_some()
            && s.wbits <= 15
            && (s.wsize == 0 || s.wsize == 1 << s.wbits)
            && s.whave <= s.wsize
            && s.wnext < s.wsize.max(1)
            && s.bits <= 64
            && table_ok(s.lencode)
            && table_ok(s.distcode)
            && (s.next as usize) <= ENOUGH
            && s.lenbits <= 15
            && s.distbits <= 15
            && s.have <= 320
            && s.nlen <= 288
            && s.ndist <= 32
            && s.ncode <= 19
            && (s.msg as usize) < MESSAGES.len();
        if !sane {
            return None;
        }
        let (header, window) = bytes.split_at_mut(INFLATE_HEADER);
        let mut f = Inflate {
            s,
            lens: [0; 320],
            work: [0; 288],
            codes: [Code::default(); ENOUGH],
            header,
            window,
        };
        let mut at = 8 + InflateScalars::COUNT * 8;
        for l in f.lens.iter_mut() {
            *l = u16::from_le_bytes([f.header[at], f.header[at + 1]]);
            at += 2;
        }
        for code in f.codes.iter_mut() {
            code.op = f.header[at];
            code.bits = f.header[at + 1];
            code.val = u16::from_le_bytes([f.header[at + 2], f.header[at + 3]]);
            at += 4;
        }
        Some(f)
    }

    fn save(&mut self) {
        self.s.write(&mut self.header[8..]);
        let mut at = 8 + InflateScalars::COUNT * 8;
        for l in self.lens {
            self.header[at..at + 2].copy_from_slice(&l.to_le_bytes());
            at += 2;
        }
        for code in self.codes {
            self.header[at] = code.op;
            self.header[at + 1] = code.bits;
            self.header[at + 2..at + 4].copy_from_slice(&code.val.to_le_bytes());
            at += 4;
        }
    }

    fn load_strm(&self, z: &mut Strm) {
        z.total_in = self.s.total_in;
        z.total_out = self.s.total_out;
        z.adler = self.s.adler;
        z.data_type = self.s.data_type;
        z.msg = self.s.msg;
    }

    fn store_strm(&mut self, z: &Strm) {
        self.s.total_in = z.total_in;
        self.s.total_out = z.total_out;
        self.s.adler = z.adler;
        self.s.data_type = z.data_type;
        self.s.msg = z.msg;
    }

    fn mode(&self) -> Mode {
        Mode::from(self.s.mode).unwrap_or(Mode::Bad)
    }

    fn set_mode(&mut self, mode: Mode) {
        self.s.mode = mode as u8;
    }

    fn code(&self, table: u32, index: u32) -> Code {
        let found = if table == LENFIX_AT {
            LENFIX.get(index as usize)
        } else if table == DISTFIX_AT {
            DISTFIX.get(index as usize)
        } else {
            self.codes.get(table as usize + index as usize)
        };
        found.copied().unwrap_or(Code {
            op: 64,
            bits: 1,
            val: 0,
        })
    }

    fn reset_keep(&mut self, z: &mut Strm) {
        z.total_in = 0;
        z.total_out = 0;
        self.s.total = 0;
        z.msg = 0;
        if self.s.wrap != 0 {
            z.adler = (self.s.wrap & 1) as u64;
        }
        self.set_mode(Mode::Head);
        self.s.last = 0;
        self.s.havedict = 0;
        self.s.flags = -1;
        self.s.dmax = 32768;
        self.s.hold = 0;
        self.s.bits = 0;
        self.s.lencode = 0;
        self.s.distcode = 0;
        self.s.next = 0;
        self.s.sane = 1;
        self.s.back = -1;
    }

    fn reset(&mut self, z: &mut Strm) {
        self.s.wsize = 0;
        self.s.whave = 0;
        self.s.wnext = 0;
        self.reset_keep(z);
    }

    fn updatewindow(&mut self, src: &[u8]) {
        // `end - copy`: the last `src.len()` bytes written, here `src`.
        let mut copy = src.len() as u32;
        if self.s.wsize == 0 {
            self.s.wsize = 1 << self.s.wbits;
            self.s.wnext = 0;
            self.s.whave = 0;
        }
        let wsize = self.s.wsize as usize;
        if copy >= self.s.wsize {
            self.window[..wsize].copy_from_slice(&src[src.len() - wsize..]);
            self.s.wnext = 0;
            self.s.whave = self.s.wsize;
        } else {
            let dist = (self.s.wsize - self.s.wnext).min(copy);
            let wnext = self.s.wnext as usize;
            let start = src.len() - copy as usize;
            self.window[wnext..wnext + dist as usize]
                .copy_from_slice(&src[start..start + dist as usize]);
            copy -= dist;
            if copy != 0 {
                let start = src.len() - copy as usize;
                self.window[..copy as usize].copy_from_slice(&src[start..]);
                self.s.wnext = copy;
                self.s.whave = self.s.wsize;
            } else {
                self.s.wnext += dist;
                if self.s.wnext == self.s.wsize {
                    self.s.wnext = 0;
                }
                if self.s.whave < self.s.wsize {
                    self.s.whave += dist;
                }
            }
        }
    }

    fn update_check(&self, check: u64, buf: &[u8]) -> u64 {
        if self.s.flags != 0 {
            crc32(check, Some(buf))
        } else {
            adler32(check, Some(buf))
        }
    }

    fn inflate(&mut self, z: &mut Strm, flush: i32) -> i32 {
        const ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        if z.null_out {
            return Z_STREAM_ERROR;
        }
        if self.mode() == Mode::Type {
            self.set_mode(Mode::Typedo);
        }
        // LOAD()
        let input = z.input;
        let out_start = z.out.len();
        let mut next = z.next_in;
        let mut have = z.avail_in();
        let mut left = z.avail_out;
        let mut hold = self.s.hold;
        let mut bits = self.s.bits;
        let inn = have;
        let mut out = left;
        let mut ret = Z_OK;

        macro_rules! pullbyte {
            ($l:lifetime) => {{
                if have == 0 {
                    break $l;
                }
                have -= 1;
                hold += u64::from(input.get(next).copied().unwrap_or(0)) << bits;
                next += 1;
                bits += 8;
            }};
        }
        macro_rules! needbits {
            ($l:lifetime, $n:expr) => {{
                while bits < ($n) as u32 {
                    pullbyte!($l);
                }
            }};
        }
        macro_rules! getbits {
            ($n:expr) => {
                ((hold as u32) & ((1u32 << ($n)) - 1))
            };
        }
        macro_rules! dropbits {
            ($n:expr) => {{
                let n = ($n) as u32;
                hold >>= n;
                bits -= n;
            }};
        }
        macro_rules! initbits {
            () => {{
                hold = 0;
                bits = 0;
            }};
        }
        macro_rules! bad {
            ($m:expr) => {{
                z.msg = $m;
                self.set_mode(Mode::Bad);
            }};
        }

        'leave: loop {
            match self.mode() {
                Mode::Head => {
                    if self.s.wrap == 0 {
                        self.set_mode(Mode::Typedo);
                        continue;
                    }
                    needbits!('leave, 16);
                    if (self.s.wrap & 2) != 0 && hold == 0x8b1f {
                        if self.s.wbits == 0 {
                            self.s.wbits = 15;
                        }
                        self.s.check = crc32(0, None);
                        self.s.check = crc32(self.s.check, Some(&[hold as u8, (hold >> 8) as u8]));
                        initbits!();
                        self.set_mode(Mode::Flags);
                        continue;
                    }
                    if (self.s.wrap & 1) == 0 || ((getbits!(8) << 8) + (hold >> 8) as u32) % 31 != 0
                    {
                        bad!(msg::HEADER_CHECK);
                        continue;
                    }
                    if getbits!(4) != Z_DEFLATED as u32 {
                        bad!(msg::METHOD);
                        continue;
                    }
                    dropbits!(4);
                    let len = getbits!(4) + 8;
                    if self.s.wbits == 0 {
                        self.s.wbits = len;
                    }
                    if len > 15 || len > self.s.wbits {
                        bad!(msg::WINDOW_SIZE);
                        continue;
                    }
                    self.s.dmax = 1 << len;
                    self.s.flags = 0;
                    self.s.check = adler32(0, None);
                    z.adler = self.s.check;
                    self.set_mode(if hold & 0x200 != 0 {
                        Mode::Dictid
                    } else {
                        Mode::Type
                    });
                    initbits!();
                }
                Mode::Flags => {
                    needbits!('leave, 16);
                    self.s.flags = hold as i32;
                    if (self.s.flags & 0xff) != Z_DEFLATED {
                        bad!(msg::METHOD);
                        continue;
                    }
                    if self.s.flags & 0xe000 != 0 {
                        bad!(msg::HEADER_FLAGS);
                        continue;
                    }
                    if (self.s.flags & 0x0200) != 0 && (self.s.wrap & 4) != 0 {
                        self.s.check = crc32(self.s.check, Some(&[hold as u8, (hold >> 8) as u8]));
                    }
                    initbits!();
                    self.set_mode(Mode::Time);
                }
                Mode::Time => {
                    needbits!('leave, 32);
                    if (self.s.flags & 0x0200) != 0 && (self.s.wrap & 4) != 0 {
                        let h = hold as u32;
                        self.s.check = crc32(self.s.check, Some(&h.to_le_bytes()));
                    }
                    initbits!();
                    self.set_mode(Mode::Os);
                }
                Mode::Os => {
                    needbits!('leave, 16);
                    if (self.s.flags & 0x0200) != 0 && (self.s.wrap & 4) != 0 {
                        self.s.check = crc32(self.s.check, Some(&[hold as u8, (hold >> 8) as u8]));
                    }
                    initbits!();
                    self.set_mode(Mode::Exlen);
                }
                Mode::Exlen => {
                    if self.s.flags & 0x0400 != 0 {
                        needbits!('leave, 16);
                        self.s.length = hold as u32;
                        if (self.s.flags & 0x0200) != 0 && (self.s.wrap & 4) != 0 {
                            self.s.check =
                                crc32(self.s.check, Some(&[hold as u8, (hold >> 8) as u8]));
                        }
                        initbits!();
                    }
                    self.set_mode(Mode::Extra);
                }
                Mode::Extra => {
                    if self.s.flags & 0x0400 != 0 {
                        let copy = self.s.length.min(have);
                        if copy != 0 {
                            if (self.s.flags & 0x0200) != 0 && (self.s.wrap & 4) != 0 {
                                self.s.check =
                                    crc32(self.s.check, Some(&input[next..next + copy as usize]));
                            }
                            have -= copy;
                            next += copy as usize;
                            self.s.length -= copy;
                        }
                        if self.s.length != 0 {
                            break 'leave;
                        }
                    }
                    self.s.length = 0;
                    self.set_mode(Mode::Name);
                }
                Mode::Name | Mode::Comment => {
                    let flag = if self.mode() == Mode::Name {
                        0x0800
                    } else {
                        0x1000
                    };
                    if self.s.flags & flag != 0 {
                        if have == 0 {
                            break 'leave;
                        }
                        let mut copy = 0usize;
                        let mut len;
                        loop {
                            len = input[next + copy];
                            copy += 1;
                            if !(len != 0 && (copy as u32) < have) {
                                break;
                            }
                        }
                        if (self.s.flags & 0x0200) != 0 && (self.s.wrap & 4) != 0 {
                            self.s.check = crc32(self.s.check, Some(&input[next..next + copy]));
                        }
                        have -= copy as u32;
                        next += copy;
                        if len != 0 {
                            break 'leave;
                        }
                    }
                    self.s.length = 0;
                    self.set_mode(if self.mode() == Mode::Name {
                        Mode::Comment
                    } else {
                        Mode::Hcrc
                    });
                }
                Mode::Hcrc => {
                    if self.s.flags & 0x0200 != 0 {
                        needbits!('leave, 16);
                        if (self.s.wrap & 4) != 0 && hold != (self.s.check & 0xffff) {
                            bad!(msg::HEADER_CRC);
                            continue;
                        }
                        initbits!();
                    }
                    self.s.check = crc32(0, None);
                    z.adler = self.s.check;
                    self.set_mode(Mode::Type);
                }
                Mode::Dictid => {
                    needbits!('leave, 32);
                    self.s.check = u64::from((hold as u32).swap_bytes());
                    z.adler = self.s.check;
                    initbits!();
                    self.set_mode(Mode::Dict);
                }
                Mode::Dict => {
                    if self.s.havedict == 0 {
                        // RESTORE() and return.
                        z.next_in = next;
                        z.avail_out = left;
                        self.s.hold = hold;
                        self.s.bits = bits;
                        return Z_NEED_DICT;
                    }
                    self.s.check = adler32(0, None);
                    z.adler = self.s.check;
                    self.set_mode(Mode::Type);
                }
                Mode::Type | Mode::Typedo => {
                    if self.mode() == Mode::Type && (flush == Z_BLOCK || flush == Z_TREES) {
                        break 'leave;
                    }
                    if self.s.last != 0 {
                        // BYTEBITS()
                        hold >>= bits & 7;
                        bits -= bits & 7;
                        self.set_mode(Mode::Check);
                        continue;
                    }
                    needbits!('leave, 3);
                    self.s.last = getbits!(1) as i32;
                    dropbits!(1);
                    match getbits!(2) {
                        0 => self.set_mode(Mode::Stored),
                        1 => {
                            self.s.lencode = LENFIX_AT;
                            self.s.lenbits = 9;
                            self.s.distcode = DISTFIX_AT;
                            self.s.distbits = 5;
                            self.set_mode(Mode::Len_);
                            if flush == Z_TREES {
                                dropbits!(2);
                                break 'leave;
                            }
                        }
                        2 => self.set_mode(Mode::Table),
                        _ => bad!(msg::BLOCK_TYPE),
                    }
                    dropbits!(2);
                }
                Mode::Stored => {
                    hold >>= bits & 7;
                    bits -= bits & 7;
                    needbits!('leave, 32);
                    if (hold & 0xffff) != ((hold >> 16) ^ 0xffff) {
                        bad!(msg::STORED_LENGTHS);
                        continue;
                    }
                    self.s.length = (hold & 0xffff) as u32;
                    initbits!();
                    self.set_mode(Mode::Copy_);
                    if flush == Z_TREES {
                        break 'leave;
                    }
                }
                Mode::Copy_ => self.set_mode(Mode::Copy),
                Mode::Copy => {
                    let mut copy = self.s.length;
                    if copy != 0 {
                        copy = copy.min(have).min(left);
                        if copy == 0 {
                            break 'leave;
                        }
                        z.out.extend_from_slice(&input[next..next + copy as usize]);
                        have -= copy;
                        next += copy as usize;
                        left -= copy;
                        self.s.length -= copy;
                        continue;
                    }
                    self.set_mode(Mode::Type);
                }
                Mode::Table => {
                    needbits!('leave, 14);
                    self.s.nlen = getbits!(5) + 257;
                    dropbits!(5);
                    self.s.ndist = getbits!(5) + 1;
                    dropbits!(5);
                    self.s.ncode = getbits!(4) + 4;
                    dropbits!(4);
                    if self.s.nlen > 286 || self.s.ndist > 30 {
                        bad!(msg::TOO_MANY_SYMBOLS);
                        continue;
                    }
                    self.s.have = 0;
                    self.set_mode(Mode::Lenlens);
                }
                Mode::Lenlens => {
                    while self.s.have < self.s.ncode {
                        needbits!('leave, 3);
                        self.lens[ORDER[self.s.have as usize]] = getbits!(3) as u16;
                        self.s.have += 1;
                        dropbits!(3);
                    }
                    while self.s.have < 19 {
                        self.lens[ORDER[self.s.have as usize]] = 0;
                        self.s.have += 1;
                    }
                    self.s.next = 0;
                    self.s.lencode = 0;
                    self.s.lenbits = 7;
                    let mut at = 0usize;
                    let lens = self.lens;
                    let r = inflate_table(
                        CodeType::Codes,
                        &lens[..19],
                        &mut self.codes,
                        &mut at,
                        &mut self.s.lenbits,
                        &mut self.work,
                    );
                    self.s.next = at as u32;
                    if r != 0 {
                        bad!(msg::CODE_LENGTHS);
                        continue;
                    }
                    self.s.have = 0;
                    self.set_mode(Mode::Codelens);
                }
                Mode::Codelens => {
                    while self.s.have < self.s.nlen + self.s.ndist {
                        let mut here;
                        loop {
                            here = self.code(self.s.lencode, getbits!(self.s.lenbits));
                            if u32::from(here.bits) <= bits {
                                break;
                            }
                            pullbyte!('leave);
                        }
                        if here.val < 16 {
                            dropbits!(here.bits);
                            self.lens[self.s.have as usize] = here.val;
                            self.s.have += 1;
                        } else {
                            let len;
                            let copy;
                            if here.val == 16 {
                                needbits!('leave, u32::from(here.bits) + 2);
                                dropbits!(here.bits);
                                if self.s.have == 0 {
                                    bad!(msg::BIT_LENGTH_REPEAT);
                                    break;
                                }
                                len = self.lens[self.s.have as usize - 1];
                                copy = 3 + getbits!(2);
                                dropbits!(2);
                            } else if here.val == 17 {
                                needbits!('leave, u32::from(here.bits) + 3);
                                dropbits!(here.bits);
                                len = 0;
                                copy = 3 + getbits!(3);
                                dropbits!(3);
                            } else {
                                needbits!('leave, u32::from(here.bits) + 7);
                                dropbits!(here.bits);
                                len = 0;
                                copy = 11 + getbits!(7);
                                dropbits!(7);
                            }
                            if self.s.have + copy > self.s.nlen + self.s.ndist {
                                bad!(msg::BIT_LENGTH_REPEAT);
                                break;
                            }
                            for _ in 0..copy {
                                self.lens[self.s.have as usize] = len;
                                self.s.have += 1;
                            }
                        }
                    }
                    if self.mode() == Mode::Bad {
                        continue;
                    }
                    if self.lens[256] == 0 {
                        bad!(msg::MISSING_END);
                        continue;
                    }
                    let lens = self.lens;
                    let nlen = self.s.nlen as usize;
                    let ndist = self.s.ndist as usize;
                    let mut at = 0usize;
                    self.s.lencode = 0;
                    self.s.lenbits = 9;
                    let r = inflate_table(
                        CodeType::Lens,
                        &lens[..nlen],
                        &mut self.codes,
                        &mut at,
                        &mut self.s.lenbits,
                        &mut self.work,
                    );
                    if r != 0 {
                        self.s.next = at as u32;
                        bad!(msg::LITLEN_SET);
                        continue;
                    }
                    self.s.distcode = at as u32;
                    self.s.distbits = 6;
                    let r = inflate_table(
                        CodeType::Dists,
                        &lens[nlen..nlen + ndist],
                        &mut self.codes,
                        &mut at,
                        &mut self.s.distbits,
                        &mut self.work,
                    );
                    self.s.next = at as u32;
                    if r != 0 {
                        bad!(msg::DISTANCES_SET);
                        continue;
                    }
                    self.set_mode(Mode::Len_);
                    if flush == Z_TREES {
                        break 'leave;
                    }
                }
                Mode::Len_ => self.set_mode(Mode::Len),
                Mode::Len => {
                    if have >= 6 && left >= 258 {
                        // RESTORE(); inflate_fast(); LOAD();
                        let (n, h, b, l, hv) = self
                            .inflate_fast(z, input, next, have, left, hold, bits, out, out_start);
                        next = n;
                        hold = h;
                        bits = b;
                        left = l;
                        have = hv;
                        if self.mode() == Mode::Type {
                            self.s.back = -1;
                        }
                        continue;
                    }
                    self.s.back = 0;
                    let mut here;
                    loop {
                        here = self.code(self.s.lencode, getbits!(self.s.lenbits));
                        if u32::from(here.bits) <= bits {
                            break;
                        }
                        pullbyte!('leave);
                    }
                    if here.op != 0 && (here.op & 0xf0) == 0 {
                        let last = here;
                        loop {
                            let i = u32::from(last.val)
                                + (getbits!(u32::from(last.bits) + u32::from(last.op))
                                    >> last.bits);
                            here = self.code(self.s.lencode, i);
                            if u32::from(last.bits) + u32::from(here.bits) <= bits {
                                break;
                            }
                            pullbyte!('leave);
                        }
                        dropbits!(last.bits);
                        self.s.back += i32::from(last.bits);
                    }
                    dropbits!(here.bits);
                    self.s.back += i32::from(here.bits);
                    self.s.length = u32::from(here.val);
                    if here.op == 0 {
                        self.set_mode(Mode::Lit);
                        continue;
                    }
                    if here.op & 32 != 0 {
                        self.s.back = -1;
                        self.set_mode(Mode::Type);
                        continue;
                    }
                    if here.op & 64 != 0 {
                        bad!(msg::LITLEN_CODE);
                        continue;
                    }
                    self.s.extra = u32::from(here.op) & 15;
                    self.set_mode(Mode::Lenext);
                }
                Mode::Lenext => {
                    if self.s.extra != 0 {
                        needbits!('leave, self.s.extra);
                        self.s.length += getbits!(self.s.extra);
                        dropbits!(self.s.extra);
                        self.s.back += self.s.extra as i32;
                    }
                    self.s.was = self.s.length;
                    self.set_mode(Mode::Dist);
                }
                Mode::Dist => {
                    let mut here;
                    loop {
                        here = self.code(self.s.distcode, getbits!(self.s.distbits));
                        if u32::from(here.bits) <= bits {
                            break;
                        }
                        pullbyte!('leave);
                    }
                    if (here.op & 0xf0) == 0 {
                        let last = here;
                        loop {
                            let i = u32::from(last.val)
                                + (getbits!(u32::from(last.bits) + u32::from(last.op))
                                    >> last.bits);
                            here = self.code(self.s.distcode, i);
                            if u32::from(last.bits) + u32::from(here.bits) <= bits {
                                break;
                            }
                            pullbyte!('leave);
                        }
                        dropbits!(last.bits);
                        self.s.back += i32::from(last.bits);
                    }
                    dropbits!(here.bits);
                    self.s.back += i32::from(here.bits);
                    if here.op & 64 != 0 {
                        bad!(msg::DISTANCE_CODE);
                        continue;
                    }
                    self.s.offset = u32::from(here.val);
                    self.s.extra = u32::from(here.op) & 15;
                    self.set_mode(Mode::Distext);
                }
                Mode::Distext => {
                    if self.s.extra != 0 {
                        needbits!('leave, self.s.extra);
                        self.s.offset += getbits!(self.s.extra);
                        dropbits!(self.s.extra);
                        self.s.back += self.s.extra as i32;
                    }
                    self.set_mode(Mode::Match);
                }
                Mode::Match => {
                    if left == 0 {
                        break 'leave;
                    }
                    let mut copy = out - left;
                    if self.s.offset > copy {
                        copy = self.s.offset - copy;
                        if copy > self.s.whave && self.s.sane != 0 {
                            bad!(msg::TOO_FAR);
                            continue;
                        }
                        let from = if copy > self.s.wnext {
                            copy -= self.s.wnext;
                            (self.s.wsize - copy) as usize
                        } else {
                            (self.s.wnext - copy) as usize
                        };
                        if copy > self.s.length {
                            copy = self.s.length;
                        }
                        if copy > left {
                            copy = left;
                        }
                        left -= copy;
                        self.s.length -= copy;
                        for k in 0..copy as usize {
                            let b = self.window.get(from + k).copied().unwrap_or(0);
                            z.out.push(b);
                        }
                    } else {
                        copy = self.s.length.min(left);
                        left -= copy;
                        self.s.length -= copy;
                        let offset = self.s.offset as usize;
                        for _ in 0..copy {
                            let b = z.out.len().checked_sub(offset).map_or(0, |i| z.out[i]);
                            z.out.push(b);
                        }
                    }
                    if self.s.length == 0 {
                        self.set_mode(Mode::Len);
                    }
                }
                Mode::Lit => {
                    if left == 0 {
                        break 'leave;
                    }
                    z.out.push(self.s.length as u8);
                    left -= 1;
                    self.set_mode(Mode::Len);
                }
                Mode::Check => {
                    if self.s.wrap != 0 {
                        needbits!('leave, 32);
                        out -= left;
                        z.total_out += u64::from(out);
                        self.s.total += u64::from(out);
                        if (self.s.wrap & 4) != 0 && out != 0 {
                            let end = z.out.len();
                            let written = &z.out[end - out as usize..end];
                            self.s.check = self.update_check(self.s.check, written);
                            z.adler = self.s.check;
                        }
                        out = left;
                        let want = if self.s.flags != 0 {
                            hold
                        } else {
                            u64::from((hold as u32).swap_bytes())
                        };
                        if (self.s.wrap & 4) != 0 && want != self.s.check {
                            bad!(msg::DATA_CHECK);
                            continue;
                        }
                        initbits!();
                    }
                    self.set_mode(Mode::Length);
                }
                Mode::Length => {
                    if self.s.wrap != 0 && self.s.flags != 0 {
                        needbits!('leave, 32);
                        if (self.s.wrap & 4) != 0 && hold != (self.s.total & 0xffff_ffff) {
                            bad!(msg::LENGTH_CHECK);
                            continue;
                        }
                        initbits!();
                    }
                    self.set_mode(Mode::Done);
                }
                Mode::Done => {
                    ret = Z_STREAM_END;
                    break 'leave;
                }
                Mode::Bad => {
                    ret = Z_DATA_ERROR;
                    break 'leave;
                }
                Mode::Mem => return Z_MEM_ERROR,
                Mode::Sync => return Z_STREAM_ERROR,
            }
        }
        // inf_leave: RESTORE()
        z.next_in = next;
        z.avail_out = left;
        self.s.hold = hold;
        self.s.bits = bits;
        let written = out - z.avail_out;
        if self.s.wsize != 0
            || (written != 0
                && self.mode() < Mode::Bad
                && (self.mode() < Mode::Check || flush != Z_FINISH))
        {
            let end = z.out.len();
            let src = z.out[end - written as usize..end].to_vec();
            self.updatewindow(&src);
        }
        let inn = inn - z.avail_in();
        let out = out - z.avail_out;
        z.total_in += u64::from(inn);
        z.total_out += u64::from(out);
        self.s.total += u64::from(out);
        if (self.s.wrap & 4) != 0 && out != 0 {
            let end = z.out.len();
            let written = &z.out[end - out as usize..end];
            self.s.check = self.update_check(self.s.check, written);
            z.adler = self.s.check;
        }
        let mode = self.mode();
        z.data_type = self.s.bits as i32
            + if self.s.last != 0 { 64 } else { 0 }
            + if mode == Mode::Type { 128 } else { 0 }
            + if mode == Mode::Len_ || mode == Mode::Copy_ {
                256
            } else {
                0
            };
        if ((inn == 0 && out == 0) || flush == Z_FINISH) && ret == Z_OK {
            ret = Z_BUF_ERROR;
        }
        ret
    }

    /// `inflate_fast`, over the locals `inflate` keeps: answers them back.
    #[allow(clippy::too_many_arguments)]
    fn inflate_fast(
        &mut self,
        z: &mut Strm,
        input: &[u8],
        next: usize,
        have: u32,
        left: u32,
        hold: u64,
        bits: u32,
        start: u32,
        out_start: usize,
    ) -> (usize, u64, u32, u32, u32) {
        let mut inp = next;
        let last = next + (have as usize - 5);
        let beg = z.out.len() - (start - left) as usize;
        let end_room = left - 257;
        let out_end = z.out.len() + end_room as usize;
        let _ = out_start;
        let wsize = self.s.wsize;
        let whave = self.s.whave;
        let wnext = self.s.wnext;
        let mut hold = hold;
        let mut bits = bits;
        let lcode = self.s.lencode;
        let dcode = self.s.distcode;
        let lmask = (1u32 << self.s.lenbits) - 1;
        let dmask = (1u32 << self.s.distbits) - 1;
        let byte = |at: usize| u64::from(input.get(at).copied().unwrap_or(0));
        let out = &mut z.out;
        'outer: loop {
            if bits < 15 {
                hold += byte(inp) << bits;
                inp += 1;
                bits += 8;
                hold += byte(inp) << bits;
                inp += 1;
                bits += 8;
            }
            let mut here = self.code(lcode, (hold as u32) & lmask);
            loop {
                // dolen
                let op = u32::from(here.bits);
                hold >>= op;
                bits -= op;
                let op = u32::from(here.op);
                if op == 0 {
                    out.push(here.val as u8);
                    break;
                } else if op & 16 != 0 {
                    let mut len = u32::from(here.val);
                    let op = op & 15;
                    if op != 0 {
                        if bits < op {
                            hold += byte(inp) << bits;
                            inp += 1;
                            bits += 8;
                        }
                        len += (hold as u32) & ((1u32 << op) - 1);
                        hold >>= op;
                        bits -= op;
                    }
                    if bits < 15 {
                        hold += byte(inp) << bits;
                        inp += 1;
                        bits += 8;
                        hold += byte(inp) << bits;
                        inp += 1;
                        bits += 8;
                    }
                    let mut here = self.code(dcode, (hold as u32) & dmask);
                    loop {
                        // dodist
                        let op = u32::from(here.bits);
                        hold >>= op;
                        bits -= op;
                        let op = u32::from(here.op);
                        if op & 16 != 0 {
                            let mut dist = u32::from(here.val);
                            let op = op & 15;
                            if bits < op {
                                hold += byte(inp) << bits;
                                inp += 1;
                                bits += 8;
                                if bits < op {
                                    hold += byte(inp) << bits;
                                    inp += 1;
                                    bits += 8;
                                }
                            }
                            dist += (hold as u32) & ((1u32 << op) - 1);
                            hold >>= op;
                            bits -= op;
                            let produced = (out.len() - beg) as u32;
                            if dist > produced {
                                let op = dist - produced;
                                if op > whave && self.s.sane != 0 {
                                    z.msg = msg::TOO_FAR;
                                    self.set_mode(Mode::Bad);
                                    break 'outer;
                                }
                                // Copy `op` bytes from the window, then the
                                // rest from the output. The C's three cases
                                // read the window circularly; so does this.
                                let mut op = op;
                                let mut from = if wnext == 0 {
                                    (wsize - op) as usize
                                } else if wnext < op {
                                    (wsize + wnext - op) as usize
                                } else {
                                    (wnext - op) as usize
                                };
                                if op < len {
                                    len -= op;
                                    while op > 0 {
                                        let b = self.window.get(from).copied().unwrap_or(0);
                                        out.push(b);
                                        from += 1;
                                        if from == wsize as usize {
                                            from = 0;
                                        }
                                        op -= 1;
                                    }
                                    let at = out.len() - dist as usize;
                                    for k in 0..len as usize {
                                        let b = out[at + k];
                                        out.push(b);
                                    }
                                } else {
                                    for _ in 0..len {
                                        let b = self.window.get(from).copied().unwrap_or(0);
                                        out.push(b);
                                        from += 1;
                                        if from == wsize as usize {
                                            from = 0;
                                        }
                                    }
                                }
                            } else {
                                let at = out.len() - dist as usize;
                                for k in 0..len as usize {
                                    let b = out[at + k];
                                    out.push(b);
                                }
                            }
                            break;
                        } else if (op & 64) == 0 {
                            here = self.code(
                                dcode,
                                u32::from(here.val) + ((hold as u32) & ((1u32 << op) - 1)),
                            );
                            continue;
                        } else {
                            z.msg = msg::DISTANCE_CODE;
                            self.set_mode(Mode::Bad);
                            break 'outer;
                        }
                    }
                    break;
                } else if (op & 64) == 0 {
                    here = self.code(
                        lcode,
                        u32::from(here.val) + ((hold as u32) & ((1u32 << op) - 1)),
                    );
                    continue;
                } else if op & 32 != 0 {
                    self.set_mode(Mode::Type);
                    break 'outer;
                } else {
                    z.msg = msg::LITLEN_CODE;
                    self.set_mode(Mode::Bad);
                    break 'outer;
                }
            }
            if !(inp < last && out.len() < out_end) {
                break;
            }
        }
        let len = bits >> 3;
        inp -= len as usize;
        bits -= len << 3;
        hold &= (1u64 << bits) - 1;
        let have = input.len().min(next + have as usize).saturating_sub(inp) as u32;
        let left = (out_end + 257).saturating_sub(out.len()) as u32;
        (inp, hold, bits, left, have)
    }

    fn set_dictionary(&mut self, z: &mut Strm, dictionary: &[u8]) -> i32 {
        if self.s.wrap != 0 && self.mode() != Mode::Dict {
            return Z_STREAM_ERROR;
        }
        if self.mode() == Mode::Dict {
            let dictid = adler32(adler32(0, None), Some(dictionary));
            if dictid != self.s.check {
                return Z_DATA_ERROR;
            }
        }
        self.updatewindow(dictionary);
        self.s.havedict = 1;
        let _ = z;
        Z_OK
    }

    fn sync(&mut self, z: &mut Strm) -> i32 {
        fn syncsearch(have: &mut u32, buf: &[u8]) -> usize {
            let mut got = *have;
            let mut next = 0;
            while next < buf.len() && got < 4 {
                if u32::from(buf[next]) == if got < 2 { 0 } else { 0xff } {
                    got += 1;
                } else if buf[next] != 0 {
                    got = 0;
                } else {
                    got = 4 - got;
                }
                next += 1;
            }
            *have = got;
            next
        }
        if z.avail_in() == 0 && self.s.bits < 8 {
            return Z_BUF_ERROR;
        }
        if self.mode() != Mode::Sync {
            self.set_mode(Mode::Sync);
            self.s.hold >>= self.s.bits & 7;
            self.s.bits -= self.s.bits & 7;
            let mut buf = [0u8; 8];
            let mut len = 0;
            while self.s.bits >= 8 && len < 8 {
                buf[len] = self.s.hold as u8;
                len += 1;
                self.s.hold >>= 8;
                self.s.bits -= 8;
            }
            self.s.have = 0;
            let mut have = 0;
            syncsearch(&mut have, &buf[..len]);
            self.s.have = have;
        }
        let mut have = self.s.have;
        let len = syncsearch(&mut have, &z.input[z.next_in..]);
        self.s.have = have;
        z.next_in += len;
        z.total_in += len as u64;
        if self.s.have != 4 {
            return Z_DATA_ERROR;
        }
        if self.s.flags == -1 {
            self.s.wrap = 0;
        } else {
            self.s.wrap &= !4;
        }
        let flags = self.s.flags;
        let (inn, out) = (z.total_in, z.total_out);
        self.reset(z);
        z.total_in = inn;
        z.total_out = out;
        self.s.flags = flags;
        self.set_mode(Mode::Type);
        Z_OK
    }
}

/// `inflateInit2`: a new stream's state bytes, or zlib's error.
pub fn inflate_init(window_bits: i32) -> Result<Vec<u8>, i32> {
    let mut window_bits = window_bits;
    let wrap;
    if window_bits < 0 {
        if window_bits < -15 {
            return Err(Z_STREAM_ERROR);
        }
        wrap = 0;
        window_bits = -window_bits;
    } else {
        wrap = (window_bits >> 4) + 5;
        if window_bits < 48 {
            window_bits &= 15;
        }
    }
    if window_bits != 0 && !(8..=15).contains(&window_bits) {
        return Err(Z_STREAM_ERROR);
    }
    let mut bytes = vec![0u8; INFLATE_SIZE];
    bytes[..8].copy_from_slice(INFLATE_MAGIC);
    let s = InflateScalars {
        wrap,
        wbits: window_bits as u32,
        ..InflateScalars::default()
    };
    s.write(&mut bytes[8..]);
    let Some(mut f) = Inflate::open(&mut bytes) else {
        return Err(Z_STREAM_ERROR);
    };
    let mut z = Strm::new(&[], 0);
    f.reset(&mut z);
    f.store_strm(&z);
    f.save();
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// The calls the primitives make.

/// Which stream these state bytes are.
pub fn is_deflate(state: &[u8]) -> bool {
    state.starts_with(DEFLATE_MAGIC)
}

/// What a call answers: zlib's return, the input it read, and the stream's
/// counters after it.
pub struct Outcome {
    pub err: i32,
    pub consumed: usize,
    pub output: Vec<u8>,
    pub msg: u8,
}

/// One `deflate()` or `inflate()` call over `input` with room for
/// `avail_out` bytes (None: `next_out` is NULL). None when the state bytes
/// are not a stream's.
pub fn run(state: &mut [u8], input: &[u8], avail_out: Option<u32>, flush: i32) -> Option<Outcome> {
    let mut z = Strm::new(input, avail_out.unwrap_or(0));
    z.null_out = avail_out.is_none();
    let err = if is_deflate(state) {
        let mut d = Deflate::open(state)?;
        d.load_strm(&mut z);
        let err = d.deflate(&mut z, flush);
        d.store_strm(&z);
        d.save();
        err
    } else {
        let mut f = Inflate::open(state)?;
        f.load_strm(&mut z);
        let err = if !(0..=Z_TREES).contains(&flush) {
            Z_STREAM_ERROR
        } else {
            f.inflate(&mut z, flush)
        };
        f.store_strm(&z);
        f.save();
        err
    };
    Some(Outcome {
        err,
        consumed: z.next_in,
        output: z.out,
        msg: z.msg,
    })
}

/// `deflateReset` or `inflateReset`.
pub fn reset(state: &mut [u8]) -> Option<i32> {
    let mut z = Strm::new(&[], 0);
    if is_deflate(state) {
        let mut d = Deflate::open(state)?;
        d.load_strm(&mut z);
        d.reset(&mut z);
        d.store_strm(&z);
        d.save();
    } else {
        let mut f = Inflate::open(state)?;
        f.load_strm(&mut z);
        f.reset(&mut z);
        f.store_strm(&z);
        f.save();
    }
    Some(Z_OK)
}

/// `deflateEnd`'s answer: Z_DATA_ERROR for a stream ended mid-way.
pub fn end(state: &mut [u8]) -> Option<i32> {
    if is_deflate(state) {
        let d = Deflate::open(state)?;
        Some(if d.s.status == BUSY_STATE {
            Z_DATA_ERROR
        } else {
            Z_OK
        })
    } else {
        Inflate::open(state)?;
        Some(Z_OK)
    }
}

/// `deflateSetDictionary` or `inflateSetDictionary`, and the stream's msg.
pub fn set_dictionary(state: &mut [u8], dictionary: &[u8]) -> Option<(i32, u8)> {
    let mut z = Strm::new(&[], 0);
    let err = if is_deflate(state) {
        let mut d = Deflate::open(state)?;
        d.load_strm(&mut z);
        let err = d.set_dictionary(&mut z, dictionary);
        d.store_strm(&z);
        d.save();
        err
    } else {
        let mut f = Inflate::open(state)?;
        f.load_strm(&mut z);
        let err = f.set_dictionary(&mut z, dictionary);
        f.store_strm(&z);
        f.save();
        err
    };
    Some((err, z.msg))
}

/// `deflateParams`, which may write a block into `avail_out` bytes.
pub fn params(
    state: &mut [u8],
    level: i32,
    strategy: i32,
    avail_out: Option<u32>,
) -> Option<Outcome> {
    let mut d = Deflate::open(state)?;
    let mut z = Strm::new(&[], avail_out.unwrap_or(0));
    z.null_out = avail_out.is_none();
    d.load_strm(&mut z);
    let err = d.params(&mut z, level, strategy);
    d.store_strm(&z);
    d.save();
    Some(Outcome {
        err,
        consumed: 0,
        output: z.out,
        msg: z.msg,
    })
}

/// `inflateSync` over `input`.
pub fn sync(state: &mut [u8], input: &[u8]) -> Option<Outcome> {
    let mut f = Inflate::open(state)?;
    let mut z = Strm::new(input, 0);
    f.load_strm(&mut z);
    let err = f.sync(&mut z);
    f.store_strm(&z);
    f.save();
    Some(Outcome {
        err,
        consumed: z.next_in,
        output: Vec::new(),
        msg: z.msg,
    })
}

/// `inflateSyncPoint`.
pub fn sync_point(state: &mut [u8]) -> Option<i32> {
    let f = Inflate::open(state)?;
    Some(i32::from(f.mode() == Mode::Stored && f.s.bits == 0))
}

/// `[total_in, total_out, adler, data_type]`.
pub fn info(state: &mut [u8]) -> Option<[u64; 4]> {
    if is_deflate(state) {
        let d = Deflate::open(state)?;
        Some([d.s.total_in, d.s.total_out, d.s.adler, d.s.data_type as u64])
    } else {
        let f = Inflate::open(state)?;
        Some([
            f.s.total_in,
            f.s.total_out,
            f.s.adler,
            f.s.data_type as i64 as u64,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How big the inputs are, and how many: under miri, where every state
    /// is a few hundred kilobytes zeroed and copied, a sample of each.
    const SCALE: usize = if cfg!(miri) { 1 } else { 100 };

    fn deflate_all(data: &[u8], level: i32, wbits: i32, mem: i32, strategy: i32) -> Vec<u8> {
        let mut state = deflate_init(level, 8, wbits, mem, strategy).unwrap();
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            let o = run(&mut state, &data[at..], Some(1024), Z_FINISH).unwrap();
            at += o.consumed;
            out.extend_from_slice(&o.output);
            if o.err == Z_STREAM_END {
                break;
            }
            assert!(
                o.err == Z_OK || o.err == Z_BUF_ERROR,
                "deflate answered {}",
                o.err
            );
        }
        out
    }

    fn inflate_all(data: &[u8], wbits: i32, room: u32) -> Result<Vec<u8>, (i32, u8)> {
        let mut state = inflate_init(wbits).unwrap();
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            let o = run(&mut state, &data[at..], Some(room), Z_SYNC_FLUSH).unwrap();
            at += o.consumed;
            out.extend_from_slice(&o.output);
            match o.err {
                Z_STREAM_END => return Ok(out),
                Z_OK => {}
                err => return Err((err, o.msg)),
            }
        }
    }

    #[test]
    fn the_spec_bytes() {
        let out = deflate_all(&[b'1'; 10], -1, 15, 8, 0);
        assert_eq!(out, [120, 156, 51, 52, 132, 1, 0, 10, 145, 1, 235]);
        assert_eq!(inflate_all(&out, 15, 7).unwrap(), [b'1'; 10]);
    }

    #[test]
    fn round_trips_every_level_and_strategy() {
        let mut data = Vec::new();
        let mut x: u32 = 1;
        for i in 0..(2_000 * SCALE) as u32 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            data.push(if i % 7 < 4 {
                b"abcdefgh"[(x >> 16) as usize % 8]
            } else {
                (x >> 24) as u8
            });
        }
        let levels: &[i32] = if cfg!(miri) {
            &[0, 1, 9]
        } else {
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        };
        let strategies: &[i32] = if cfg!(miri) {
            &[0, 2]
        } else {
            &[0, 1, 2, 3, 4]
        };
        let windows: &[i32] = if cfg!(miri) { &[15] } else { &[9, 15, -15, 31] };
        for &level in levels {
            for &strategy in strategies {
                for &wbits in windows {
                    let out = deflate_all(&data, level, wbits, 8, strategy);
                    assert_eq!(
                        inflate_all(&out, wbits, 300).unwrap(),
                        data,
                        "{level} {strategy} {wbits}"
                    );
                }
            }
        }
    }

    #[test]
    fn checksums() {
        assert_eq!(adler32(1, Some(b"foo")), 0x0282_0145);
        assert_eq!(crc32(0, Some(b"foo")), 0x8c73_6521);
        // A sum past 32 bits, and one at or past BASE, as zlib reads them.
        assert_eq!(crc32(0xffff_ffff, Some(b"")), 0xffff_ffff);
        assert_eq!(adler32(0xffff_ffff, Some(b"")), (14 << 16) | 14);
        assert_eq!(adler32(1, None), 1);
        assert_eq!(crc32(5, None), 0);
    }

    #[test]
    fn corrupt_input_never_panics() {
        // A bounded fuzz: mutate valid streams and feed random bytes in
        // random chunk sizes; every call must answer, and none may write
        // past its room.
        let sample: Vec<u8> = (0..(50 * SCALE) as u32)
            .map(|i| (i * 7 % 251) as u8 ^ (i >> 5) as u8)
            .collect();
        let mut seeds = Vec::new();
        for level in [0, 1, 6, 9] {
            seeds.push(deflate_all(&sample, level, 15, 8, 0));
            seeds.push(deflate_all(&sample, level, 31, 8, 0));
        }
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut rand = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for round in 0..15 * SCALE {
            let mut data = seeds[round % seeds.len()].clone();
            if round % 5 == 4 {
                let n = (rand() % 600) as usize;
                data = (0..n).map(|_| rand() as u8).collect();
            } else {
                for _ in 0..1 + rand() % 6 {
                    let i = (rand() as usize) % data.len();
                    data[i] = rand() as u8;
                }
                if rand() % 4 == 0 {
                    data.truncate((rand() as usize) % data.len());
                }
            }
            let wbits = [15, 31, 47, -15, 0][(rand() % 5) as usize];
            let mut state = inflate_init(wbits).unwrap();
            let mut at = 0;
            for _ in 0..10_000 {
                let chunk = 1 + (rand() % 700) as usize;
                let room = 1 + (rand() % 2000) as u32;
                let end = (at + chunk).min(data.len());
                let o = run(&mut state, &data[at..end], Some(room), Z_SYNC_FLUSH).unwrap();
                assert!(o.output.len() <= room as usize);
                at += o.consumed;
                if o.err != Z_OK || (at == data.len() && o.output.is_empty()) {
                    break;
                }
            }
        }
    }
}
