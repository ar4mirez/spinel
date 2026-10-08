//! What a method *is*: the table behind [`Method::body`].
//!
//! [#8](https://github.com/ar4mirez/spinel/issues/8) left `Method::body` as an
//! opaque [`Value`] because there was no bytecode yet. This is what it points
//! at: an index into a per-heap table whose entries are either an [`Iseq`] or
//! one of the handful of operations Ruby cannot define in Ruby.
//!
//! # Why a fixnum id and not a heap object
//!
//! A heap object would need a payload kind that can hold an `Arc<Iseq>` and a
//! finaliser to drop one, and the heap has neither — [`Payload`] is slots or
//! bytes, and the collector sweeps without running destructors. A definition id
//! is a fixnum, so the collector never has to trace a method body at all, and
//! the table it indexes is per-heap and dropped with the heap.
//!
//! [`Method::body`]: crate::class::Method::body
//! [`Payload`]: crate::heap::Payload

use std::sync::Arc;

use crate::bytecode::Iseq;
use crate::value::{SymbolId, Value};

/// Which of `Module`'s four class-variable reflection methods is being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CvarOp {
    Get,
    Set,
    Defined,
    /// `Module#class_variables`: own first, then inherited. Measured.
    Names,
}

/// Which of `Object`'s four instance-variable reflection methods is being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvarOp {
    Get,
    Set,
    Defined,
    /// `Object#instance_variables`, in the order the object acquired them.
    Names,
}

/// An operation the VM performs itself.
///
/// engine.md's rule for what becomes a primitive is "raw memory, allocation,
/// encoding tables, syscalls, **dispatch**, and anything the JIT needs as an
/// intrinsic". Every entry here is dispatch: calling a block, forwarding a call
/// under another name, or reading a `Proc`'s own shape. The rest of `Kernel`
/// is Ruby and waits for
/// [#15](https://github.com/ar4mirez/spinel/issues/15).
///
/// An enum rather than a function pointer because two of these — [`Native::Call`]
/// and [`Native::Send`] — do not *return* a value, they push a frame, and a
/// function that could do that would need the whole interpreter as an argument.
/// The loop matches on this instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    /// `Proc#call`, and its aliases `()`, `[]`, and `yield`. Pushes a frame.
    Call,
    /// `Object#send`, `__send__`, `public_send`. Re-dispatches under the name
    /// in the first argument. Pushes a frame when the target is Ruby.
    Send {
        public_only: bool,
    },
    /// `Kernel#proc`, `Kernel#lambda`, `Proc.new`. Returns the block it was
    /// passed; `lambda` also marks it one.
    MakeProc {
        lambda: bool,
    },
    /// `Proc#lambda?`
    IsLambda,
    /// `Proc#arity`
    Arity,
    /// `Kernel#block_given?`
    BlockGiven,
    /// `Object#class`
    ClassOf,
    /// `Class#new`: allocate, then run `initialize` if there is one.
    ///
    /// A primitive because it is allocation and dispatch, which `docs/engine.md`
    /// reserves for Rust. Everything else `Class` will answer is `core/*.rb`'s.
    New,
    /// `Object#equal?` — identity, which for this VM is `Value` equality.
    Equal,
    /// `Array#+`: a new array with the two joined. Allocation, so Rust.
    ArrayPlus,
    /// Read slot `n` of the receiver. A fixed slot, so only for the built-ins
    /// whose representation *is* fixed: `MatchData`'s regexp and subject.
    Getter(u16),
    /// Write slot `n` of the receiver, answering the value written.
    Setter(u16),
    /// Read the receiver's instance variable, by name. What `attr_reader`
    /// defines.
    ///
    /// By name and not by slot, because which slot an instance variable lands
    /// at is the object's shape's business: an `attr_reader` built on slot 0
    /// would be right for a class with one ivar and wrong for its second. #15
    /// left `attr_accessor` out rather than ship that.
    IvarReader(SymbolId),
    /// Write the receiver's instance variable, by name, answering the value
    /// written. What `attr_writer` defines.
    IvarWriter(SymbolId),
    /// `Module#attr_reader`, `#attr_writer`, `#attr_accessor`. Defines an
    /// [`Native::IvarReader`], an [`Native::IvarWriter`], or both per name, and
    /// answers the array of symbols it defined — which is what Ruby 3.0 does.
    AttrDefine {
        reader: bool,
        writer: bool,
    },
    /// `Object#instance_variable_get`, `#instance_variable_set`,
    /// `#instance_variable_defined?` and `#instance_variables`.
    InstanceVariable(IvarOp),
    /// `Kernel#raise` and `Kernel#fail`. Does not return a value: it hands the
    /// interpreter an unwind, which is why it lives here and not in Ruby.
    Raise,
    /// `Kernel#throw`. The other primitive that unwinds.
    Throw,
    /// `Kernel#catch`. Pushes a frame for the block and marks it as the
    /// boundary a matching `throw` stops at.
    Catch,
    /// `Regexp#=~` — the character offset the match began at, or nil.
    RegexpMatchOp,
    /// `Regexp#match` — a `MatchData`, or nil.
    RegexpMatch,
    /// `Regexp#match?` — a boolean, and the one matcher that leaves `$~` alone.
    RegexpMatchP,
    /// `Regexp#===`, which is what `when /re/` runs.
    RegexpCaseEq,
    /// `Regexp#source`
    RegexpSource,
    /// `Regexp#options`
    RegexpOptions,
    /// `Regexp#to_s` (`(?-mix:foo)`) and `#inspect` (`/foo/`), which differ.
    RegexpToS {
        inspect: bool,
    },
    /// `String#=~`, `String#match`, `String#match?` — the same three matchers
    /// with the operands the other way round.
    StringMatchOp,
    StringMatch,
    StringMatchP,
    /// `MatchData#[]`, by group number or by capture name.
    MatchIndex,
    /// `MatchData#to_a` and `#captures`, which differ only in whether the whole
    /// match is the first element.
    MatchToA {
        captures: bool,
    },
    /// `MatchData#pre_match` and `#post_match`.
    MatchAround {
        post: bool,
    },
    /// `MatchData#begin` and `#end`, in characters.
    MatchEdge {
        end: bool,
    },
    /// `MatchData#size` and `#length`.
    MatchSize,
    /// `MatchData#names` — the capture names the pattern declares, in group
    /// order and without repeats. What `MatchData#inspect` branches on.
    MatchNames,
    /// `Object#frozen?`, `Object#nil?`, `Object#!`. Cheap predicates the target
    /// specs reach for while checking something else.
    NilP,

    // -- #15's core library. Each one is raw memory or allocation; everything
    // -- else about these classes is Ruby, in `core/*.rb`.
    /// `Array#[]` — reads a raw slot run, or copies one out for a slice.
    ArrayIndex,
    /// `Array#at` — one index, never a slice (#21).
    ArrayIndexSingle,
    /// `Array#[]=` — writes one, and reallocates storage past the end.
    ArrayStore,
    /// `Array#size` — reads the length slot.
    ArraySize,
    /// `Array#push` — writes a raw slot, reallocating storage when full.
    ArrayPush,
    /// `Array#pop` — writes the length back.
    ArrayPop,
    /// `String#length` and `#size` (characters), and `#bytesize` (bytes).
    /// One primitive, because both read the same byte payload's length; they
    /// differ only in whether the bytes are decoded first.
    StringSize {
        bytes: bool,
    },
    /// `String#+` — allocates a byte payload.
    StringConcat,
    /// `String#*` — allocates a byte payload.
    StringRepeat,
    /// `Class#allocate` — allocation, and the shape is per class.
    Allocate,
    /// `Object#dup` — allocates a copy of a cell. `Array` overrides it in Ruby,
    /// because a shallow copy of an `Array` would share its storage object.
    Dup,
    /// `Object#freeze` — sets a header flag bit.
    Freeze,
    /// `Object#frozen?` — reads it.
    FrozenP,
    /// `Object#object_id` — the object's address.
    ObjectId,
    /// `Integer#<<`, `#>>`, `#&`, `#|`, `#^`, `#~` — fixnum bit patterns, which
    /// the JIT wants as intrinsics.
    IntBits(BitOp),
    /// `Integer#+` and `Float#+`, and the other eight arithmetic and relational
    /// operators: the method an explicit `2.send(:+, 1)` finds (#239). The same
    /// function [`Insn::BinOp`][crate::bytecode::Insn::BinOp] answers from, with
    /// `Numeric`'s coercing operator behind it as `super` when that declines.
    NumOp(crate::bytecode::BinOp),
    /// `Integer#-@` and `Float#-@`.
    NumNeg,
    /// `Integer#**` — repeated multiplication with an overflow check, so the
    /// answer is a refusal rather than a wrapped one.
    IntPow,
    /// `Integer#__to_s_radix__(base)`: the digits in `base`, 2 to 36, as a
    /// US-ASCII String. A primitive because building them in Ruby divides the
    /// whole number once per digit, which is quadratic on a bignum.
    IntToSRadix,
    /// `Symbol#to_s`, `#name`, `#length` — reads the shared symbol table.
    SymbolName {
        length: bool,
    },
    /// `Module#name`, `Module#to_s` — reads the class table.
    ModuleName,
    /// `Module#private_constant` — marks names in the module's own constant
    /// table invisible to a qualified reference (#185).
    ///
    /// A primitive because the visibility it records is read by the constant
    /// lookup, which is the class table's own walk and not something Ruby code
    /// can reach.
    PrivateConstant,
    /// `Object#hash` — a fixnum that is equal whenever `==` is.
    ///
    /// Content for a `String` and an `Array`, identity for everything else,
    /// which is Ruby's own default. A primitive because it reads raw bytes and
    /// raw slots, and because a `Hash` keyed on it wants it as an intrinsic.
    HashValue,
    /// `Module#include` and `Module#prepend` — splices a module into the
    /// ancestor chain, which is a write to the class table.
    ///
    /// Without it `core/comparable.rb` is unreachable: `include Comparable` is
    /// how every mixin in Ruby is used.
    Mixin {
        prepend: bool,
    },
    /// `Kernel#respond_to?` — a lookup from the receiver's *dispatch* class.
    ///
    /// Not `self.class.method_defined?`, which is what `core/kernel.rb` had:
    /// `Object#class` skips the singleton, by design, so that spelling could
    /// never see a `def obj.foo` or an `extend`ed module. The question
    /// `respond_to?` asks is the one dispatch asks, so it starts where dispatch
    /// starts.
    RespondTo,
    /// `Object#extend` — an `include` into the receiver's singleton class,
    /// which is the whole of what Ruby's `extend` is.
    ///
    /// A primitive rather than `singleton_class.include(m)` in Ruby, because
    /// `Module#include` is private in Ruby and the singleton class of an
    /// ordinary object is allocated by the *table*, not by anything `core/*.rb`
    /// can reach.
    Extend,
    /// `Module#ancestors` — the linearised chain, which only the class table
    /// knows. `is_a?`, `kind_of?`, `Module#===` and `Module#<` are Ruby on it.
    Ancestors,
    /// `Module#class_variables`, `#class_variable_get`, `#class_variable_set`
    /// and `#class_variable_defined?` — reflection over the same per-class
    /// table `@@a` reads and writes.
    ClassVariable(CvarOp),
    /// `Module#alias_method`, the send-shaped spelling of the `alias`
    /// statement — same table copy, on the receiver rather than on the frame's
    /// definee. Answers the new name, measured.
    AliasMethod,
    /// `Module#undef_method`. Writes the tombstone `undef` writes, and answers
    /// the module. Measured.
    UndefMethod,
    /// `Class#superclass` — one step up the same chain.
    Superclass,
    /// `Module#private`, `#public`, `#protected` (#161).
    ///
    /// Bare, it sets the visibility the `def`s below it in the body get, which
    /// lives on the lexical scope; with arguments it sets each named method's
    /// and answers the arguments, so `private def m; end` works because `def`
    /// answers a symbol.
    SetVisibility(crate::class::Visibility),
    /// `Module#module_function`, with arguments (#161).
    ///
    /// Two definitions, which is what the name hides: the instance method
    /// becomes private, and a *public* copy lands on the module's singleton.
    /// That is why `Kernel.puts` answers and `Object.print` does not, and it
    /// cannot be modelled by visibility alone.
    ModuleFunction,
    /// `Module#private_method_defined?` and its two siblings (#161).
    VisibilityDefined(crate::class::Visibility),
    /// `Module#method_defined?` — a method-table lookup.
    MethodDefined,
    /// `Float#to_s` — the shortest decimal that reads back as the same float,
    /// which is an algorithm (Ruby uses `dtoa`) and not a formatting rule.
    FloatToS,
    /// `String#[]` — allocates a substring out of a byte payload.
    StringIndex,
    /// `String#<=>` — compares two byte payloads.
    StringCompare,
    /// Writes a `String`'s bytes to stdout. A syscall, so Rust.
    ///
    /// Installed as `Kernel#__write__`, which is not a Ruby method name.
    /// `docs/engine.md` spells a primitive `Primitive.write(...)`; there is no
    /// `Primitive` module yet, and inventing one for a single entry would be a
    /// module to name, bootstrap and document before anything needed it.
    /// `puts`, `print` and `p` are Ruby on top of this.
    WriteString,
    /// `strerror(3)` for an error number, as a String (#29).
    ///
    /// Installed as `Kernel#__strerror__`. It is the platform's message table,
    /// which Ruby cannot reach: `SystemCallError#initialize` is Ruby on top of
    /// it, and so is every `Errno::E*` default message.
    Strerror,
    /// The `Errno::E*` class for an error number, or nil (#29).
    ///
    /// Installed as `Kernel#__errno_class__`. `SystemCallError.new(msg, 2)`
    /// answers an `Errno::ENOENT`, and which class a number belongs to is the
    /// platform's table rather than anything Ruby can enumerate.
    ErrnoClass,
    /// The platform's signal table, as `[[name, number], ...]` in CRuby's
    /// order, followed by `NSIG` (#29).
    ///
    /// Installed as `Kernel#__signal_list__`. `Signal.list` and
    /// `SignalException#initialize` are Ruby on top of it.
    SignalList,
    /// The backtrace of the frames below the calling one, as
    /// `[[path, line, label], ...]` innermost first (#29).
    ///
    /// Installed as `Kernel#__backtrace_here__`. The frame stack is the VM's,
    /// so reading it is a primitive; `caller`, `caller_locations` and
    /// `full_message`'s fallback position are Ruby on top. The calling frame —
    /// the Ruby method that asked — is left out, so `caller(0)` written in
    /// Ruby starts where Ruby's does.
    BacktraceHere,
    /// Whether standard error is a terminal: `Exception.to_tty?` (#29). A
    /// syscall, so Rust.
    StderrTty,
    /// A path made absolute and resolved, or nil when it names no file:
    /// `Thread::Backtrace::Location#absolute_path` (#29). A syscall, so Rust.
    AbsolutePath,
    /// `Kernel#__method__` and, with `callee`, `#__callee__` (#28): the
    /// running method's name, which only the frames know.
    FrameMethod {
        callee: bool,
    },
    /// `Kernel#__dir__`: the directory of the file the caller was written in.
    FrameDir,
    /// `Module.nesting`: the caller's lexical scopes, which only its frame
    /// knows.
    FrameNesting,
    /// `binding`, string `eval` and a `Binding`'s locals (#38).
    Binding(BindingOp),
    /// `Kernel#__load_file__(path, wrap)`: parse, compile and run a file at
    /// the top level, in a frame of its own (#39). `require` and `load` are
    /// Ruby around it.
    LoadFile,
    /// `Kernel#__refusal_boundary__(limit) { ... }`: run the block with a
    /// budget of `limit` instructions. A refusal inside it — something this
    /// VM cannot run yet, or running past the budget — ends the block and
    /// answers the reason as a String instead of ending the evaluation. No
    /// `rescue` sees it. A spec runner wraps each example in one (#145).
    RefusalBoundary,
    /// `Kernel#__freeze_global__(*names)`: those globals refuse assignment.
    FreezeGlobal,
    /// `__hook_global__(reads, *names)`: assignments to each name, and reads
    /// when `reads`, go through `Kernel#__global_assign__` and
    /// `#__global_read__`.
    HookGlobal,
    /// `__global_store__(name, value)`: the cell itself, past any hook.
    GlobalStore,
    /// `__global_fetch__(name)`: the cell itself, nil when never assigned.
    GlobalFetch,
    /// `Kernel#__argv__`: `[$0, *ARGV]` as the embedder set them.
    Argv,
    /// `Kernel#__mark_partial__`: a file this heap was meant to load did not
    /// finish. See `HandleScope::mark_partial`.
    MarkPartial,
    /// `Kernel#__ruby_constants__`: `RUBY_VERSION` and its neighbours, as a
    /// flat `[name, value, ...]` Array, from the constants `--version` prints.
    RubyConstants,
    /// `Kernel#__environ__`: the process environment, as a flat
    /// `[name, value, ...]` Array.
    Environ,
    /// `Kernel#__getenv__(name)`: one variable, or nil.
    Getenv,
    /// The file system calls `File` and `Dir` are Ruby over (#39).
    Fs(FsOp),
    /// The process's identity and clocks, for `Process` (#145).
    Sys(SysOp),
    /// `Kernel#__sleep__(seconds)`: block the thread, answer the whole
    /// seconds slept. `Kernel#sleep` is Ruby around it.
    Sleep,
    /// `Kernel#__needs_threads__`: the VM declining to start a thread (#45).
    ///
    /// A refusal rather than a Ruby `NotImplementedError`, because a spec that
    /// asserts `Thread.new` raises `ThreadError` would read a Ruby exception as
    /// a wrong answer. This reads as "cannot be answered yet", which it is.
    NeedsThreads,
    /// A primitive that only refuses, for a method whose real work belongs to
    /// a later issue: `fork` before `Process` (#43).
    Refuse {
        what: &'static str,
        needs: &'static str,
    },
    /// `Kernel#__hash_combine__(a, b)`: two hash values mixed into one (#22).
    ///
    /// The bit-mixing a content hash needs, which Ruby has no primitive for.
    /// `Array#hash` and `Hash#hash` are Ruby folds over their elements' own
    /// `hash` methods on top of it, so a key class with a custom `hash` is
    /// honoured inside an Array or a Hash too.
    HashCombine,
    /// The fiber primitives (#16), installed as `Kernel#__fiber_*__` and
    /// wrapped by `core/fiber.rb`. Each that switches fibers does it by
    /// swapping the vectors the interpreter loop runs on.
    Fiber(FiberOp),
    /// `Kernel#__proc_location__(proc)`: `[path, line]` where a block was
    /// written, or nil — what `Fiber#inspect` names (#16) and
    /// `Proc#source_location` will.
    ProcLocation,
    /// `Module#define_method`, and `Kernel#define_singleton_method` when
    /// `singleton` (#28): a `Proc` becomes a method body.
    DefineMethod {
        singleton: bool,
    },
    /// `instance_eval`/`instance_exec` (`module: false`) and
    /// `class_eval`/`module_eval`/`class_exec`/`module_exec` (`module: true`)
    /// with a block (#28). The `_exec` forms pass their arguments; the `_eval`
    /// forms pass the receiver. A String body is #38's.
    EvalBlock {
        module: bool,
        exec: bool,
    },
    /// Reads and writes of the class table that `core/module.rb` and
    /// `core/kernel.rb` build reflection on (#28). Installed as
    /// `Kernel#__reflect_*__`.
    Reflect(ReflectOp),
    /// The `String` and `Encoding` primitives `core/string.rb` and
    /// `core/encoding.rb` build on (#19). Bytes and encoding indexes only; every
    /// rule about which encoding wins, and every argument check, is Ruby.
    Str(StrOp),
    /// `String#to_sym` and `#intern`: the symbol table is the VM's.
    StringIntern,
    /// `Kernel#__raise_no_method__(name, args)`: the NoMethodError the VM
    /// raises for a missing method, raised on purpose — what
    /// `BasicObject#method_missing` does when called directly (#28).
    RaiseNoMethod,
}

/// Which file system call. Each answers an Integer errno on failure, for the
/// Ruby side to raise as `SystemCallError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsOp {
    /// `__fs_kind__(path, follow)`: `:file`, `:directory`, `:link`, `:other`,
    /// or nil when there is nothing there.
    Kind,
    /// `__fs_realpath__(path)`: every link resolved.
    Realpath,
    /// `__fs_getcwd__`.
    Getcwd,
    /// `__fs_isatty__(fd)`.
    Isatty,
    /// `__fs_access__(path, mode)`: whether `access(2)` allows it, for
    /// `File.readable?`, `writable?` and `executable?`.
    Access,
    /// `__fs_chdir__(path)`: true.
    Chdir,
    /// `__fs_read__(path)`: the bytes, as a BINARY String.
    Read,
    /// `__fs_children__(path)`: a directory's entry names, sorted, without
    /// `.` and `..`.
    Children,
    /// `__fs_constants__`: `[name, value]` pairs for `File::Constants`, from
    /// the target's libc.
    Constants,
}

/// Which process query. See `interp.rs`, `sys_native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysOp {
    /// `__sys_ids__`: `[pid, ppid, uid, euid, gid, egid]`.
    Ids,
    /// `__sys_clock__(id)`: `[seconds, nanoseconds]` of that `clockid_t`, or
    /// an errno.
    Clock,
    /// `__sys_clock_ids__`: `[name, id]` pairs for `Process::CLOCK_*`.
    ClockIds,
    /// `__sys_process_constants__`: `[name, value]` pairs for the rest of
    /// `Process`'s constants — `WNOHANG`, `PRIO_*`, `RLIMIT_*`, `RLIM_*`.
    ProcessConstants,
}

/// Which `Binding` operation. See `interp.rs`, `binding_native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingOp {
    /// `Kernel#binding`: the caller's frame. With `caller`, the frame below
    /// it — what `Kernel#eval`, written in Ruby, was called from.
    Capture { caller: bool },
    /// `Binding#__eval__(source, file, line)`: compile against the binding's
    /// locals and run in a frame inside its environment.
    Eval,
    /// `Binding#__local_get__(name)`: the value, or `undefined` when there
    /// is no such local — the Ruby side raises.
    Get,
    /// `Binding#__local_set__(name, value)`, declaring it if new.
    Set,
    /// `Binding#local_variables`, innermost first, without duplicates.
    Names,
    /// `Binding#receiver`.
    Receiver,
    /// `Binding#__binding_receiver_set__(self)`: `Kernel.eval` runs with
    /// its receiver as `self`.
    SetReceiver,
    /// `Binding#source_location`.
    Location,
}

/// Which `String`/`Encoding` operation. See `interp.rs`, `str_native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrOp {
    /// `()` → the receiver's encoding index.
    EncodingIndex,
    /// `(index)` → the receiver, now tagged with that encoding. No
    /// conversion: `force_encoding` only relabels.
    ForceEncoding,
    /// `(byte_start, byte_len, string)` → the receiver, with those bytes
    /// replaced by the string's: every in-place change is one of these.
    Splice,
    /// `(index)` → the byte there, or nil past either end.
    GetByte,
    /// `(index, byte)` → the byte, written; the index is already checked.
    SetByte,
    /// `(byte_start, byte_len)` → a new String of those bytes, same encoding;
    /// the range is already clamped.
    ByteSlice,
    /// `()` → every byte, as an Array of Integers.
    Bytes,
    /// `()` → whether the bytes are valid in the receiver's encoding.
    ValidEncoding,
    /// `()` → whether the receiver is ASCII in an ASCII-compatible encoding.
    AsciiOnly,
    /// `()` → each character's starting byte offset, then the bytesize.
    CharOffsets,
    /// `(string)` → the encoding index the two would combine in, or nil.
    Compatible,
    /// `(needle, byte_start)` → the byte offset of the first `needle` at or
    /// after `byte_start`, or nil.
    ByteIndex,
    /// `(needle, byte_start)` → the byte offset of the last `needle` starting
    /// at or before `byte_start`, or nil.
    ByteRindex,
    /// `(kind, ascii_only, turkic)` → a new String, case-mapped: kind 0 is
    /// upcase, 1 downcase, 2 swapcase, 3 capitalize, 4 fold. Unicode for a
    /// UTF-8 string, ASCII for US-ASCII and BINARY.
    CaseMap,
    /// `()` → never answers: `pack`/`unpack`'s `p` and `P` read and write
    /// raw pointers, which a Spinel program has no way to hold.
    NeedsPointers,
    /// `Float#__bits__(width)` → the IEEE 754 bits of the receiver at 32 or
    /// 64 bits, as an Integer: what `pack` writes for `e`, `g`, `d` and kin.
    FloatBits,
    /// `Integer#__float_from_bits__(width)` → the Float those bits are, at 32 or 64.
    FloatFromBits,
    /// `(source, destination, start)` → one transcoding step from byte
    /// `start`: `[output, stop, error_bytes, readagain_bytes, next,
    /// codepoint]`, stop being `:done`, `:invalid`, `:incomplete` or
    /// `:undefined`; or nil when the pair is not one this VM converts.
    Transcode,
    /// `()` → the receiver's successor, as `String#succ` defines it.
    Succ,
    /// `()` → never answers: the refusal for a character in an encoding
    /// whose table this VM does not have, raised where Ruby would otherwise
    /// have to guess (`0xA4A2.chr("EUC-JP")`).
    NeedsCharTable,
    /// `Float#__format__(conversion, precision, alternate)`: the digits of
    /// the receiver's magnitude for `%f`, `%e` or `%g` (or their capitals),
    /// as C's printf writes them. Sign, width and padding are `format`'s.
    FloatFormat,
    /// `()` on `Encoding`: build every encoding object, `Encoding::LIST`, and
    /// every constant, at once. Boot runs this per heap, and the same work as
    /// a Ruby loop was a measurable share of every spec example's start-up.
    EncodingInstall,
}

/// Which class-table operation. See `interp.rs`, `reflect_native`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectOp {
    /// `(mod, name, inherit)` → `[value]`, or nil when there is none.
    ConstLookup,
    /// `(mod, name, value)` → value, naming an anonymous module it holds.
    ConstSet,
    /// `(mod, inherit)` → the public constant names, definition order.
    ConstNames,
    /// `(mod, name)` → `[value]` removed, or nil.
    ConstRemove,
    /// `(mod, name)` → whether the module holds it; makes it public.
    ConstPublic,
    /// `(class, inherit, which)` → method names: `which` 0 is public and
    /// protected, 1 public, 2 protected, 3 private.
    MethodNames,
    /// `(object)` → its class, singleton included if it has one.
    ClassOf,
    /// `(object)` → its singleton class, made if need be.
    SingletonClass,
    /// `(mod, name)` → whether `mod` defined it and it is now gone.
    RemoveMethod,
    /// `(mod)` → whether it is a singleton class.
    IsSingleton,
    /// `(object)` → `:class`, `:module`, or nil for anything else. What
    /// `include` checks its arguments with while the core library is still
    /// loading and `is_a?` cannot yet run.
    ModuleKind,
    /// `(singleton class)` → the object it belongs to.
    Attached,
    /// `(object, name)` → the arity of the method a send would find, -1 for
    /// one with no fixed count, or nil when there is none. What `Kernel#warn`
    /// asks of `Warning.warn` before passing it a keyword.
    MethodArity,
    /// `(mod, name, mark)` → with `mark`, deprecates the constant; without,
    /// the module holding the deprecated constant that `mod.const_get(name)`
    /// finds, or nil.
    ConstDeprecated,
    /// `(from, to)` → nil; gives `to` a copy of `from`'s singleton class when
    /// it has one. `Kernel#clone`'s half that Ruby cannot reach.
    CopySingleton,
}

/// Which fiber primitive. See `interp.rs`, "Fibers".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberOp {
    New,
    Resume,
    Yield,
    Raise,
    Kill,
    Transfer,
    Current,
    Alive,
    Status,
}

/// The bitwise operators on `Integer`, which share one primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitOp {
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Not,
}

/// A method body.
#[derive(Debug, Clone)]
pub enum Definition {
    /// Compiled Ruby. `Arc` because the same body is reachable from the class
    /// table and from the `Iseq` that defined it, and because phase 3 shares
    /// bytecode between Ractors.
    Iseq(Arc<Iseq>),
    Native(Native),
    /// A `Proc` made a method by `define_method` (#28): its body runs with the
    /// receiver as `self`, a method's arity and `return`, and the method's
    /// owner for `super`. Traced by the collector through
    /// [`Definitions::each_root`], since nothing else may hold the `Proc`.
    Proc(Value),
}

/// One heap's method bodies, indexed by the fixnum in [`Method::body`].
///
/// Append-only within a heap: redefining a method points the class table at a
/// new entry rather than mutating one, so a frame already running the old body
/// keeps running it. That is Ruby's rule — redefining a method mid-call does
/// not rewrite the call in flight.
///
/// [`Method::body`]: crate::class::Method::body
#[derive(Debug, Default)]
pub struct Definitions {
    entries: Vec<Definition>,
    /// Body id per `Arc<Iseq>` address, so evaluating a block literal in a loop
    /// interns one definition rather than one per iteration. The `Iseq` is kept
    /// alive by the `Iseq` that owns it as a child, which outlives the frame
    /// that could look it up.
    interned: std::collections::HashMap<usize, Value>,
}

impl Definitions {
    #[must_use]
    pub fn new() -> Definitions {
        Definitions {
            entries: Vec::new(),
            interned: std::collections::HashMap::new(),
        }
    }

    /// The body id for a compiled `Iseq`, added once per distinct `Iseq`.
    ///
    /// `key` is the `Arc`'s address. Without the memo, `10.times { }` would add
    /// a definition per iteration and the table would grow with the loop.
    ///
    /// An address is only a safe key because the entry it points at holds a
    /// clone of the same `Arc`: the `Iseq` cannot be dropped while the table
    /// remembers it, so its address cannot be reused by a different one. That
    /// is an invariant of `add` below, not a coincidence — a memo that stored
    /// the id without keeping the `Arc` would eventually answer with the wrong
    /// method body.
    pub fn intern_iseq(&mut self, iseq: &Arc<Iseq>, key: usize) -> Value {
        if let Some(&body) = self.interned.get(&key) {
            return body;
        }
        let body = self.add(Definition::Iseq(Arc::clone(iseq)));
        self.interned.insert(key, body);
        body
    }

    /// Add a definition and return the [`Value`] that names it.
    ///
    /// # Panics
    ///
    /// If a heap ever holds more definitions than a fixnum can index, which is
    /// 2^62 of them.
    pub fn add(&mut self, definition: Definition) -> Value {
        let id = self.entries.len();
        self.entries.push(definition);
        Value::fixnum(id as i64).expect("a definition id fits a fixnum")
    }

    #[must_use]
    pub fn get(&self, body: Value) -> Option<&Definition> {
        let id = body.as_fixnum()?;
        self.entries.get(usize::try_from(id).ok()?)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Every heap value a definition holds: the `Proc`s `define_method` made
    /// into methods.
    pub fn each_root(&self, mut f: impl FnMut(Value)) {
        for definition in &self.entries {
            if let Definition::Proc(block) = definition {
                f(*block);
            }
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_id_round_trips_and_is_not_a_heap_value() {
        let mut defs = Definitions::new();
        let body = defs.add(Definition::Native(Native::Arity));
        // The whole reason for a fixnum body: the collector never traces it.
        assert!(body.is_immediate());
        assert!(matches!(
            defs.get(body),
            Some(Definition::Native(Native::Arity))
        ));
    }

    #[test]
    fn redefining_leaves_the_old_body_reachable() {
        // A frame already running the old body holds its id, and that id must
        // keep resolving after the class table has moved on.
        let mut defs = Definitions::new();
        let old = defs.add(Definition::Native(Native::Arity));
        let new = defs.add(Definition::Native(Native::IsLambda));
        assert_ne!(old, new);
        assert!(matches!(
            defs.get(old),
            Some(Definition::Native(Native::Arity))
        ));
    }
}
