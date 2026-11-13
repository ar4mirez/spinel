//! The bytecode cache (#40): compiled files in `.spinel/bytecode/`, keyed by
//! the SHA-256 of their source, so a second run of a program reads its
//! required files' bytecode back instead of parsing and compiling them.
//!
//! # What makes a cache file safe to read
//!
//! An [`Iseq`] is already position-independent (`bytecode.rs`): jumps are
//! relative, symbols are names relinked on load, and literals are
//! descriptions rather than heap pointers. So the on-disk form is a plain
//! serialisation of the tree, and what is read back is the same immutable,
//! `Send + Sync` value the compiler would have made, shareable the same way.
//!
//! What the file name does not pin, the header does. A file is used only when
//! all of these match, and is otherwise a miss that parses as if there were
//! no cache, then overwrites it:
//!
//! - the format and [`FINGERPRINT`], a hash of every source that decides
//!   what a file compiles to (`build.rs`), so a cache written by another
//!   build of Spinel is stale;
//! - the source's own hash, again, for a file renamed into place by hand;
//! - the path the file was compiled under, which `__FILE__`, `__dir__` and
//!   every backtrace line carry, and the `frozen_string_literal` setting;
//! - the length and SHA-256 of the payload, so a truncated or corrupted file
//!   is never decoded into a different program.
//!
//! The payload itself is then read with every length and tag checked, so even
//! a file that passed the checksum but does not decode is a miss, not a
//! panic. Writes go to a temporary file in the same directory and are renamed
//! into place, so a process reading concurrently sees the old file, the new
//! one, or none, never half of one.
//!
//! The parser's warnings about the file are part of the entry and are given
//! again on a hit, so `-w` reads the same with and without the cache.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::bytecode::{
    BinOp, BlockRef, CallSite, CatchEntry, CatchKind, ClassDef, ConstScope, DefKind, Insn, Iseq,
    Keyword, Literal, MatchRef, Optional, ParamSpec,
};

/// The first bytes of every cache file.
const MAGIC: &[u8; 8] = b"SPINELBC";

/// The layout of the header and payload below. The fingerprint already
/// changes whenever this file does; the number is for a reader that wants to
/// say why a file is stale.
const FORMAT: u32 = 1;

/// The compiler's identity, from `build.rs`: hex SHA-256 over the sources of
/// `spinel-vm`, `spinel-parse` and `spinel-ast` and the lock file.
pub const FINGERPRINT: &str = env!("SPINEL_BYTECODE_FINGERPRINT");

/// The deepest nesting of bodies the cache writes or reads. A real program's
/// blocks nest a handful deep; the bound keeps a hostile file from recursing
/// the reader off the end of its stack. A deeper file is compiled every time.
const MAX_DEPTH: u32 = 256;

/// What a cache hit gives back: the compiled file, and the parser's warnings
/// about it, as the [`crate::Parser`] gave them.
#[derive(Debug)]
pub struct Entry {
    pub iseq: Arc<Iseq>,
    pub warnings: Vec<(String, bool)>,
}

/// The SHA-256 of a source, which names its cache file.
#[must_use]
pub fn key(source: &[u8]) -> [u8; 32] {
    Sha256::digest(source).into()
}

/// `<dir>/<hex key>.sbc`.
// ponytail: named for the source alone, as `docs/cli.md` specifies, so two
// files with the same bytes at different paths share a name, and a run that
// loads both rewrites it for the other each time (the path is in the header).
// The upgrade, if a project shows it, is the path's hash in the name too.
#[must_use]
pub fn file_for(dir: &Path, key: &[u8; 32]) -> PathBuf {
    dir.join(format!("{}.sbc", hex(key)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The cached compile of `source`, read from `dir`, when there is one that is
/// valid for it: see the module docs for what that takes. Any other outcome —
/// no file, an unreadable one, a stale or corrupt one — is `None`, and the
/// caller parses.
#[must_use]
pub fn read(dir: &Path, path: &str, source: &[u8], frozen: Option<bool>) -> Option<Entry> {
    let key = key(source);
    let bytes = std::fs::read(file_for(dir, &key)).ok()?;
    decode(&bytes, path, &key, frozen)
}

/// Write the compile of `source` into `dir`, best effort: a cache that cannot
/// be written is a slower next run, never an error. Nothing is written for a
/// body that cannot be cached (see [`encode`]).
pub fn write(
    dir: &Path,
    path: &str,
    source: &[u8],
    frozen: Option<bool>,
    iseq: &Iseq,
    warnings: &[(String, bool)],
) {
    let key = key(source);
    let Some(bytes) = encode(iseq, warnings, path, &key, frozen) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Unique to this process and this write, in the same directory so the
    // rename cannot cross a file system.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let temporary = dir.join(format!(".{}.{}.{nanos}.tmp", hex(&key), std::process::id()));
    if std::fs::write(&temporary, &bytes).is_err()
        || std::fs::rename(&temporary, file_for(dir, &key)).is_err()
    {
        let _ = std::fs::remove_file(&temporary);
    }
}

fn frozen_byte(frozen: Option<bool>) -> u8 {
    match frozen {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

/// A whole cache file for `iseq`, compiled from the source whose hash is
/// `key` under `path`. `None` for a body that cannot be cached: one the
/// compiler refused part of, whose refusal names are the compiler's own
/// `'static` strings, or one nested deeper than [`MAX_DEPTH`].
#[must_use]
pub fn encode(
    iseq: &Iseq,
    warnings: &[(String, bool)],
    path: &str,
    key: &[u8; 32],
    frozen: Option<bool>,
) -> Option<Vec<u8>> {
    if !cacheable(iseq, 0) {
        return None;
    }
    let mut payload = Writer::default();
    iseq.put(&mut payload);
    warnings.len().put(&mut payload);
    for (text, verbose) in warnings {
        text.as_str().put(&mut payload);
        verbose.put(&mut payload);
    }
    let payload = payload.0;
    let mut out = Writer::default();
    out.0.extend_from_slice(MAGIC);
    FORMAT.put(&mut out);
    out.0.extend_from_slice(FINGERPRINT.as_bytes());
    out.0.extend_from_slice(key);
    frozen_byte(frozen).put(&mut out);
    path.put(&mut out);
    payload.len().put(&mut out);
    out.0.extend_from_slice(&Sha256::digest(&payload));
    out.0.extend_from_slice(&payload);
    Some(out.0)
}

/// The entry in a cache file, when it is one for `path`, the source whose
/// hash is `key`, and `frozen`, written by this build, and intact.
#[must_use]
pub fn decode(bytes: &[u8], path: &str, key: &[u8; 32], frozen: Option<bool>) -> Option<Entry> {
    let mut header = Reader::new(bytes);
    if header.take(MAGIC.len())? != MAGIC
        || u32::get(&mut header)? != FORMAT
        || header.take(FINGERPRINT.len())? != FINGERPRINT.as_bytes()
        || header.take(key.len())? != key
        || u8::get(&mut header)? != frozen_byte(frozen)
        || &*Box::<str>::get(&mut header)? != path
    {
        return None;
    }
    let length = usize::get(&mut header)?;
    let checksum = header.take(32)?;
    let payload = header.take(length)?;
    if !header.done() || Sha256::digest(payload).as_slice() != checksum {
        return None;
    }
    let mut reader = Reader::new(payload);
    let iseq = Iseq::get(&mut reader)?;
    let count = usize::get(&mut reader)?;
    let mut warnings = Vec::with_capacity(count.min(reader.remaining()));
    for _ in 0..count {
        let text = Box::<str>::get(&mut reader)?;
        warnings.push((text.into_string(), bool::get(&mut reader)?));
    }
    if !reader.done() {
        return None;
    }
    Some(Entry {
        iseq: Arc::new(iseq),
        warnings,
    })
}

fn cacheable(iseq: &Iseq, depth: u32) -> bool {
    depth < MAX_DEPTH
        && iseq.refusals.is_empty()
        && iseq
            .children
            .iter()
            .all(|child| cacheable(child, depth + 1))
}

// ---------------------------------------------------------------------------
// The wire format: little-endian integers, lengths as u64, a tag byte per
// enum variant. Every read is checked; a short or malformed payload is `None`.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Writer(Vec<u8>);

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    /// How many bodies deep the read is, against [`MAX_DEPTH`].
    depth: u32,
    /// The last body's path. Every body of a file shares one, so it is read
    /// into one `Arc` rather than one per body.
    path: Option<Arc<str>>,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader {
            data,
            at: 0,
            depth: 0,
            path: None,
        }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let bytes = self.data.get(self.at..end)?;
        self.at = end;
        Some(bytes)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.at
    }

    fn done(&self) -> bool {
        self.at == self.data.len()
    }
}

trait Wire: Sized {
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader<'_>) -> Option<Self>;
}

macro_rules! wire_int {
    ($($ty:ty),*) => {$(
        impl Wire for $ty {
            fn put(&self, w: &mut Writer) {
                w.0.extend_from_slice(&self.to_le_bytes());
            }
            fn get(r: &mut Reader<'_>) -> Option<Self> {
                Some(<$ty>::from_le_bytes(r.array()?))
            }
        }
    )*};
}

wire_int!(u8, u16, u32, u64, i32, i64);

impl Wire for usize {
    fn put(&self, w: &mut Writer) {
        (*self as u64).put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        usize::try_from(u64::get(r)?).ok()
    }
}

impl Wire for bool {
    fn put(&self, w: &mut Writer) {
        u8::from(*self).put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        match u8::get(r)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

/// By its bits, so `-0.0` and every NaN come back as they went.
impl Wire for f64 {
    fn put(&self, w: &mut Writer) {
        self.to_bits().put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        Some(f64::from_bits(u64::get(r)?))
    }
}

impl Wire for &str {
    fn put(&self, w: &mut Writer) {
        self.as_bytes().put(w);
    }
    fn get(_: &mut Reader<'_>) -> Option<Self> {
        // Written only: a read owns what it reads.
        None
    }
}

impl Wire for &[u8] {
    fn put(&self, w: &mut Writer) {
        self.len().put(w);
        w.0.extend_from_slice(self);
    }
    fn get(_: &mut Reader<'_>) -> Option<Self> {
        None
    }
}

impl Wire for Box<[u8]> {
    fn put(&self, w: &mut Writer) {
        (&**self).put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        let length = usize::get(r)?;
        Some(r.take(length)?.into())
    }
}

impl Wire for Box<str> {
    fn put(&self, w: &mut Writer) {
        (&**self).put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        let length = usize::get(r)?;
        Some(std::str::from_utf8(r.take(length)?).ok()?.into())
    }
}

impl Wire for String {
    fn put(&self, w: &mut Writer) {
        self.as_str().put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        Some(Box::<str>::get(r)?.into_string())
    }
}

impl Wire for Arc<str> {
    fn put(&self, w: &mut Writer) {
        (&**self).put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        let text = Box::<str>::get(r)?;
        if let Some(last) = &r.path
            && **last == *text
        {
            return Some(Arc::clone(last));
        }
        let shared: Arc<str> = text.into();
        r.path = Some(Arc::clone(&shared));
        Some(shared)
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, w: &mut Writer) {
        self.len().put(w);
        for item in self {
            item.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        let length = usize::get(r)?;
        // Every item is at least a byte, so a length past what is left is a
        // corrupt file, not an allocation to make.
        if length > r.remaining() {
            return None;
        }
        let mut items = Vec::with_capacity(length);
        for _ in 0..length {
            items.push(T::get(r)?);
        }
        Some(items)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            None => false.put(w),
            Some(item) => {
                true.put(w);
                item.put(w);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        if bool::get(r)? {
            Some(Some(T::get(r)?))
        } else {
            Some(None)
        }
    }
}

impl<A: Wire, B: Wire> Wire for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        Some((A::get(r)?, B::get(r)?))
    }
}

impl Wire for Arc<Iseq> {
    fn put(&self, w: &mut Writer) {
        (**self).put(w);
    }
    fn get(r: &mut Reader<'_>) -> Option<Self> {
        Some(Arc::new(Iseq::get(r)?))
    }
}

/// An enum, one tag byte per variant and then its fields in order. The
/// write's `match` is exhaustive, so a new variant does not compile until it
/// is listed here, and a tag listed twice is an unreachable pattern, which
/// `deny` makes an error.
macro_rules! wire_enum {
    ($ty:ident {
        $( $tag:literal => $variant:ident
            $( ( $($field:ident),* ) )?
            $( { $($named:ident),* } )?
        ),* $(,)?
    }) => {
        impl Wire for $ty {
            fn put(&self, w: &mut Writer) {
                match self {
                    $( $ty::$variant $( ( $($field),* ) )? $( { $($named),* } )? => {
                        ($tag as u8).put(w);
                        $( $( $field.put(w); )* )?
                        $( $( $named.put(w); )* )?
                    } )*
                }
            }
            #[deny(unreachable_patterns)]
            fn get(r: &mut Reader<'_>) -> Option<Self> {
                Some(match u8::get(r)? {
                    $( $tag => $ty::$variant
                        $( ( $( { let $field = Wire::get(r)?; $field } ),* ) )?
                        $( { $( $named: Wire::get(r)? ),* } )?, )*
                    _ => return None,
                })
            }
        }
    };
}

/// A struct, its fields in the order listed. Both directions name every
/// field without `..`, so a new field does not compile until it is listed.
macro_rules! wire_struct {
    ($ty:ident { $($field:ident),* $(,)? }) => {
        impl Wire for $ty {
            fn put(&self, w: &mut Writer) {
                let $ty { $($field),* } = self;
                $( $field.put(w); )*
            }
            fn get(r: &mut Reader<'_>) -> Option<Self> {
                Some($ty { $( $field: Wire::get(r)? ),* })
            }
        }
    };
}

wire_enum!(BinOp {
    0 => Add,
    1 => Sub,
    2 => Mul,
    3 => Div,
    4 => Mod,
    5 => Eq,
    6 => Neq,
    7 => Lt,
    8 => Le,
    9 => Gt,
    10 => Ge,
});

wire_enum!(MatchRef {
    0 => Data,
    1 => Group(group),
    2 => Whole,
    3 => Pre,
    4 => Post,
    5 => LastGroup,
});

wire_enum!(ConstScope {
    0 => Lexical,
    1 => Qualified,
    2 => Top,
});

wire_enum!(DefKind {
    0 => Class,
    1 => Module,
    2 => Singleton,
});

wire_enum!(CatchKind {
    0 => Rescue,
    1 => Ensure,
});

wire_enum!(BlockRef {
    0 => None,
    1 => Literal(child),
    2 => Pass,
});

wire_enum!(Literal {
    0 => BigInt(digits),
    1 => BoxedFloat(float),
    2 => Str(bytes, encoding),
    3 => FrozenStr(bytes, encoding),
    4 => ChilledStr(bytes, encoding),
    5 => Regexp { source, options, encoding, parser_warned },
});

wire_enum!(Insn {
    0 => PushNil,
    1 => PushTrue,
    2 => PushFalse,
    3 => PushSelf,
    4 => PushInt(int),
    5 => PushFloat(bits),
    6 => PushLit(index),
    7 => PushSym(index),
    8 => Intern,
    9 => Pop,
    10 => Dup,
    11 => CaptureSplat,
    12 => GetLocal(slot, depth),
    13 => SetLocal(slot, depth),
    14 => Jump(offset),
    15 => JumpUnless(offset),
    16 => JumpIf(offset),
    17 => Refuse(index),
    18 => JumpUnlessKeep(offset),
    19 => JumpIfKeep(offset),
    20 => JumpIfNilKeep(offset),
    21 => JumpUnlessUndef(offset),
    22 => BinOp(op),
    23 => Neg,
    24 => Not,
    25 => BlockToProc,
    26 => NewArray(count),
    27 => CaseEq,
    28 => Send(site),
    29 => Super(site),
    30 => Yield(site),
    31 => LeaveThroughEnsure,
    32 => MakeProc(child, lambda),
    33 => DefineMethod(index),
    34 => DefineSingleton(index),
    35 => GetConst(name, scope, site),
    36 => SetConst(name, scope),
    37 => DefinedConst(name, scope),
    38 => OpenClass(index),
    39 => GetIvar(name, site),
    40 => SetIvar(name, site),
    41 => DefinedIvar(name),
    42 => GetGlobal(name),
    43 => SetGlobal(name),
    44 => DefinedGlobal(name),
    45 => DefinedMethod(name),
    46 => DefinedSelfMethod(name),
    47 => DefinedYield,
    48 => DefinedSuper,
    49 => Leave,
    50 => Return,
    51 => Break,
    52 => Raise,
    53 => CheckMatch,
    54 => CheckMatchAny,
    55 => NewRegexp(flags),
    56 => NewRegexpOnce(flags, site),
    57 => OnceGet(offset, site),
    58 => OnceSet(site),
    59 => GetCvar(name),
    60 => SetCvar(name),
    61 => DefinedCvar(name),
    62 => Alias(new, old),
    63 => Undef(name),
    64 => AliasFromStack,
    65 => UndefFromStack,
    66 => AliasGlobal(new, old),
    67 => AliasGlobalSpecial(new, special),
    68 => LastMatch(which),
    69 => SetLastMatch,
    70 => Errinfo,
    71 => SetErrinfo,
    72 => DefinedMatch(which),
    73 => Goto(offset, site),
    74 => GotoValue(offset, site),
    75 => EnterEnsure,
    76 => LeaveEnsure,
    77 => GetLocalPushIntBinOp { slot, int, op },
    78 => GetLocalGetLocalBinOp { left, right, op },
    79 => GetLocalGetIvarBinOp { slot, site, op },
    80 => PushSelfGetLocal { slot },
    81 => GetLocalBinOp { slot, op },
});

wire_struct!(ClassDef {
    name,
    body,
    kind,
    scoped,
    superclass
});

wire_struct!(CatchEntry {
    kind,
    start,
    end,
    target,
    stack_depth
});

wire_struct!(CallSite {
    name,
    argc,
    splats,
    keywords,
    block,
    implicit_self,
    variable_call,
    kwsplat,
});

wire_struct!(Optional { slot });

wire_struct!(Keyword {
    name,
    slot,
    required
});

wire_struct!(ParamSpec {
    required,
    optional,
    rest,
    trailing_comma,
    post,
    keywords,
    kwrest,
    no_keywords,
    block,
    unnamed,
});

/// By hand rather than [`wire_struct!`] for the two fields that are not
/// written: `refusals`, which [`encode`] has already checked is empty, and
/// `linked`, a per-process cache of the symbol ids that a read starts empty,
/// which is the "relinked on load" half of position independence.
impl Wire for Iseq {
    fn put(&self, w: &mut Writer) {
        let Iseq {
            name,
            insns,
            literals,
            symbols,
            locals,
            max_stack,
            params,
            children,
            call_sites,
            definitions,
            once_regexps,
            ivar_sites,
            const_sites,
            class_defs,
            catch_table,
            scope_barrier,
            path,
            lines,
            block_level,
            first_line,
            outer,
            from_eval,
            eval_in_block,
            refusals: _,
            uses_block,
            symbol_encoding,
            linked: _,
        } = self;
        name.put(w);
        insns.put(w);
        literals.put(w);
        symbols.put(w);
        locals.put(w);
        max_stack.put(w);
        params.put(w);
        children.put(w);
        call_sites.put(w);
        definitions.put(w);
        once_regexps.put(w);
        ivar_sites.put(w);
        const_sites.put(w);
        class_defs.put(w);
        catch_table.put(w);
        scope_barrier.put(w);
        path.put(w);
        lines.put(w);
        block_level.put(w);
        first_line.put(w);
        outer.put(w);
        from_eval.put(w);
        eval_in_block.put(w);
        uses_block.put(w);
        symbol_encoding.put(w);
    }

    fn get(r: &mut Reader<'_>) -> Option<Self> {
        if r.depth >= MAX_DEPTH {
            return None;
        }
        r.depth += 1;
        let iseq = Iseq {
            name: Wire::get(r)?,
            insns: Wire::get(r)?,
            literals: Wire::get(r)?,
            symbols: Wire::get(r)?,
            locals: Wire::get(r)?,
            max_stack: Wire::get(r)?,
            params: Wire::get(r)?,
            children: Wire::get(r)?,
            call_sites: Wire::get(r)?,
            definitions: Wire::get(r)?,
            once_regexps: Wire::get(r)?,
            ivar_sites: Wire::get(r)?,
            const_sites: Wire::get(r)?,
            class_defs: Wire::get(r)?,
            catch_table: Wire::get(r)?,
            scope_barrier: Wire::get(r)?,
            path: Wire::get(r)?,
            lines: Wire::get(r)?,
            block_level: Wire::get(r)?,
            first_line: Wire::get(r)?,
            outer: Wire::get(r)?,
            from_eval: Wire::get(r)?,
            eval_in_block: Wire::get(r)?,
            refusals: Vec::new(),
            uses_block: Wire::get(r)?,
            symbol_encoding: Wire::get(r)?,
            linked: crate::shared::symbols::Linked::default(),
        };
        r.depth -= 1;
        Some(iseq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Iseq {
        let block = Iseq {
            name: "block in <top (required)>".into(),
            insns: vec![
                Insn::GetLocal(0, 1),
                Insn::GetLocalPushIntBinOp {
                    slot: 0,
                    int: -7,
                    op: BinOp::Lt,
                },
                Insn::LastMatch(MatchRef::Group(3)),
                Insn::Leave,
            ],
            params: ParamSpec {
                required: vec![0],
                optional: vec![Optional { slot: 1 }],
                rest: Some(2),
                keywords: vec![Keyword {
                    name: 0,
                    slot: 3,
                    required: true,
                }],
                ..ParamSpec::default()
            },
            symbols: vec!["k".into()],
            locals: vec!["a".into(), "b".into(), "c".into(), "k".into()],
            path: Some("/p/a.rb".into()),
            block_level: 1,
            outer: vec![vec!["x".into()]],
            ..Iseq::default()
        };
        Iseq {
            name: "<top (required)>".into(),
            insns: vec![
                Insn::PushLit(0),
                Insn::PushFloat(1.5f64.to_bits()),
                Insn::Jump(-3),
                Insn::GetConst(0, ConstScope::Top, 0),
                Insn::Send(0),
                Insn::MakeProc(0, true),
                Insn::Leave,
            ],
            literals: vec![
                Literal::BigInt("123456789012345678901234567890".into()),
                Literal::BoxedFloat(-0.0),
                Literal::Str((*b"caf\xc3\xa9").into(), 1),
                Literal::FrozenStr((*b"\xff").into(), 2),
                Literal::ChilledStr(Box::default(), 0),
                Literal::Regexp {
                    source: "a+".into(),
                    options: 5,
                    encoding: 1,
                    parser_warned: true,
                },
            ],
            symbols: vec!["puts".into(), "Foo".into()],
            max_stack: 4,
            children: vec![Arc::new(block)],
            call_sites: vec![CallSite {
                name: 0,
                argc: 2,
                splats: vec![1],
                keywords: vec![1],
                block: BlockRef::Literal(0),
                implicit_self: true,
                variable_call: false,
                kwsplat: true,
            }],
            definitions: vec![(1, 0)],
            class_defs: vec![ClassDef {
                name: 1,
                body: 0,
                kind: DefKind::Module,
                scoped: true,
                superclass: false,
            }],
            catch_table: vec![CatchEntry {
                kind: CatchKind::Ensure,
                start: 0,
                end: 3,
                target: 5,
                stack_depth: 1,
            }],
            path: Some("/p/a.rb".into()),
            lines: vec![(0, 1), (3, 2)],
            symbol_encoding: Some(3),
            uses_block: true,
            ..Iseq::default()
        }
    }

    const KEY: [u8; 32] = [7; 32];

    fn warnings() -> Vec<(String, bool)> {
        vec![("/p/a.rb:1: warning: x\n".into(), true)]
    }

    #[test]
    fn a_file_reads_back_as_the_iseq_that_was_written() {
        let iseq = sample();
        let bytes = encode(&iseq, &warnings(), "/p/a.rb", &KEY, Some(true)).unwrap();
        let entry = decode(&bytes, "/p/a.rb", &KEY, Some(true)).unwrap();
        // `Literal`'s `PartialEq` would compare the boxed `-0.0` equal to
        // `0.0`; `Debug` prints the sign, so compare that too.
        assert_eq!(*entry.iseq, iseq);
        assert_eq!(format!("{:?}", entry.iseq), format!("{iseq:?}"));
        assert_eq!(entry.warnings, warnings());
        // The child shares the parent's path rather than holding a copy.
        let parent = entry.iseq.path.as_ref().unwrap();
        let child = entry.iseq.children[0].path.as_ref().unwrap();
        assert!(Arc::ptr_eq(parent, child));
    }

    #[test]
    fn a_header_that_does_not_match_is_a_miss() {
        let bytes = encode(&sample(), &[], "/p/a.rb", &KEY, None).unwrap();
        assert!(decode(&bytes, "/p/a.rb", &KEY, None).is_some());
        assert!(decode(&bytes, "/p/b.rb", &KEY, None).is_none());
        assert!(decode(&bytes, "/p/a.rb", &[8; 32], None).is_none());
        assert!(decode(&bytes, "/p/a.rb", &KEY, Some(false)).is_none());
        // Another build's fingerprint.
        let mut other = bytes.clone();
        other[MAGIC.len() + 4] ^= 1;
        assert!(decode(&other, "/p/a.rb", &KEY, None).is_none());
    }

    #[test]
    fn every_truncation_and_every_flipped_byte_is_a_miss() {
        let bytes = encode(&sample(), &warnings(), "/p/a.rb", &KEY, None).unwrap();
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length], "/p/a.rb", &KEY, None).is_none());
        }
        // A step keeps Miri's run short; natively every byte is tried.
        let step = if cfg!(miri) { 37 } else { 1 };
        for at in (0..bytes.len()).step_by(step) {
            let mut corrupt = bytes.clone();
            corrupt[at] ^= 0x41;
            assert!(
                decode(&corrupt, "/p/a.rb", &KEY, None).is_none(),
                "flipping byte {at} went unnoticed"
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode(&longer, "/p/a.rb", &KEY, None).is_none());
    }

    #[test]
    fn a_payload_that_passes_the_checksum_but_does_not_decode_is_a_miss() {
        // Hand-built: a vector claiming more items than there are bytes, and
        // an unknown tag, each behind a correct checksum.
        for payload in [vec![0xff; 8], vec![0, 0, 0, 0, 0, 0, 0, 0, 0xee]] {
            let mut file = Writer::default();
            file.0.extend_from_slice(MAGIC);
            FORMAT.put(&mut file);
            file.0.extend_from_slice(FINGERPRINT.as_bytes());
            file.0.extend_from_slice(&KEY);
            0u8.put(&mut file);
            "/p/a.rb".put(&mut file);
            payload.len().put(&mut file);
            file.0.extend_from_slice(&Sha256::digest(&payload));
            file.0.extend_from_slice(&payload);
            assert!(decode(&file.0, "/p/a.rb", &KEY, None).is_none());
        }
    }

    #[test]
    fn a_body_with_a_refusal_or_too_deep_is_not_written() {
        let refused = Iseq {
            refusals: vec!["a construct"],
            ..Iseq::default()
        };
        assert!(encode(&refused, &[], "/p", &KEY, None).is_none());
        let mut deep = Iseq::default();
        for _ in 0..MAX_DEPTH {
            deep = Iseq {
                children: vec![Arc::new(deep)],
                ..Iseq::default()
            };
        }
        assert!(encode(&deep, &[], "/p", &KEY, None).is_none());
    }

    /// What the cache hands out is what the compiler hands out: immutable
    /// and shareable across threads, which is what a Ractor needs.
    #[test]
    fn a_read_entry_is_send_and_sync() {
        fn shareable<T: Send + Sync>() {}
        shareable::<Arc<Iseq>>();
        let bytes = encode(&sample(), &[], "/p/a.rb", &KEY, None).unwrap();
        let iseq = decode(&bytes, "/p/a.rb", &KEY, None).unwrap().iseq;
        let other = Arc::clone(&iseq);
        let linked = std::thread::spawn(move || other.link()).join().unwrap();
        assert_eq!(linked, iseq.link());
    }
}
