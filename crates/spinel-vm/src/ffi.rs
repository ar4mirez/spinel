//! `Spinel::FFI` (#105): calling C from Ruby, on libffi.
//!
//! What Ruby cannot say for `fiddle`: `dlopen(3)` and `dlsym(3)`, the C heap
//! (`malloc`, `realloc`, `free`), reading and writing bytes at an address, a
//! call through libffi, and a closure — a C function pointer that calls back
//! into Ruby. Everything above that, the types and conversions, `Fiddle` and
//! the `Spinel::FFI` module, is `lib/spinel/ffi.rb` and `lib/fiddle/spinel.rb`
//! over the `__ffi_*__` primitives `interp.rs` makes of these.
//!
//! Values cross as *slots*: eight bytes each, an integer's two's-complement
//! bits (sign- or zero-extended from its C type) or a floating-point value's
//! `f64` bits. No VM value is ever handed to C, and no address inside the
//! heap: a String's bytes reach C as a copy the Ruby side makes on the C heap.
//!
//! A callback is the hard part, because the interpreter is not re-entrant: a
//! primitive cannot run Ruby, and C calling a closure is C asking for Ruby in
//! the middle of a primitive. So while any closure is live, a call runs on a
//! stack of its own (a `corosensei` coroutine, on the same OS thread). When C
//! calls a closure, the closure's trampoline *suspends* that stack and the
//! primitive answers [`Outcome::Callback`]: which closure, with what
//! arguments. Ruby runs the closure's block — an ordinary send, in the
//! interpreter loop — and hands the answer to [`Ffi::resume`], which switches
//! back to the C stack and lets the trampoline return it to C. A block that
//! raises, breaks or throws abandons the call ([`Ffi::abandon`]): its stack is
//! dropped without running anything more on it, which is what CRuby's
//! `longjmp` out of a callback does to the C frames under it.
//!
//! A heap's FFI state lives in its [`Ffi`]; nothing here is process-global.
//!
//! A call holds the heap's lock while C runs, so a C function that blocks
//! blocks the heap's other threads.
// ponytail: CRuby's fiddle releases the GVL for a call unless `need_gvl:`
// asks otherwise. Upgrade: make the call outside the lock, as
// `threads::outside` makes a blocking system call, and take it again in the
// trampoline before suspending.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_void};
use std::sync::{Arc, Mutex, PoisonError};

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};
use libffi::low;
use libffi::middle::{Cif, Type};

// Fiddle's type codes (`fiddle.h`), which `lib/spinel/ffi.rb` uses too. A
// negative code is the unsigned form of the positive one.
pub const VOID: i64 = 0;
pub const VOIDP: i64 = 1;
pub const CHAR: i64 = 2;
pub const SHORT: i64 = 3;
pub const INT: i64 = 4;
pub const LONG: i64 = 5;
pub const LONG_LONG: i64 = 6;
pub const FLOAT: i64 = 7;
pub const DOUBLE: i64 = 8;
pub const VARIADIC: i64 = 9;
pub const CONST_STRING: i64 = 10;
pub const BOOL: i64 = 11;

/// The size of a C call's coroutine stack: what a thread gets by default on
/// Linux, so C that recurses deeply under a callback has the room it would
/// have on a thread. Mapped lazily, so the untouched part costs nothing.
const STACK_SIZE: usize = 8 << 20;

/// The libffi type for a Fiddle type code, or the code back when it names
/// no type (`RuntimeError: unknown type`, as `fiddle`'s conversions say).
pub fn ffi_type(code: i64) -> Result<Type, i64> {
    let unsigned = code < 0;
    let pick = |signed: Type, plain: Type| if unsigned { plain } else { signed };
    Ok(match code.abs() {
        VOID => Type::void(),
        VOIDP | CONST_STRING => Type::pointer(),
        CHAR => pick(Type::c_schar(), Type::c_uchar()),
        SHORT => pick(Type::c_short(), Type::c_ushort()),
        INT => pick(Type::c_int(), Type::c_uint()),
        LONG => pick(Type::c_long(), Type::c_ulong()),
        LONG_LONG => pick(Type::c_longlong(), Type::c_ulonglong()),
        FLOAT => Type::f32(),
        DOUBLE => Type::f64(),
        // `sizeof(bool)` is 1 on every platform Spinel builds for.
        BOOL => Type::u8(),
        _ => return Err(code),
    })
}

/// How many bytes a value of type `code` takes, and its alignment: what
/// `Fiddle::SIZEOF_*` and `ALIGN_*` say. `None` for `VOID` and codes that
/// name no type.
#[must_use]
pub fn layout(code: i64) -> Option<(usize, usize)> {
    use std::mem::{align_of, size_of};
    use std::os::raw::{c_char, c_int, c_long, c_longlong, c_short};
    let pair = |size, align| Some((size, align));
    match code.abs() {
        VOIDP | CONST_STRING => pair(size_of::<*const c_void>(), align_of::<*const c_void>()),
        CHAR => pair(size_of::<c_char>(), align_of::<c_char>()),
        SHORT => pair(size_of::<c_short>(), align_of::<c_short>()),
        INT => pair(size_of::<c_int>(), align_of::<c_int>()),
        LONG => pair(size_of::<c_long>(), align_of::<c_long>()),
        LONG_LONG => pair(size_of::<c_longlong>(), align_of::<c_longlong>()),
        FLOAT => pair(size_of::<f32>(), align_of::<f32>()),
        DOUBLE => pair(size_of::<f64>(), align_of::<f64>()),
        BOOL => pair(size_of::<bool>(), align_of::<bool>()),
        _ => None,
    }
}

/// libffi's status for a failed `ffi_prep_cif`, as the number `fiddle`
/// puts in its "error creating CIF %d" message.
fn status(error: &low::Error) -> i32 {
    match error {
        low::Error::Typedef => 1,
        low::Error::Abi => 2,
        low::Error::ArgType => 3,
        _ => -1,
    }
}

/// Write slot `slot` to `place` as a value of type `code`.
///
/// # Safety
///
/// `place` is valid for a write of the type's size and aligned for it.
unsafe fn store(code: i64, slot: u64, place: *mut c_void) {
    // SAFETY: the caller's contract, for each width.
    unsafe {
        match code.abs() {
            CHAR | BOOL => place.cast::<u8>().write(slot as u8),
            SHORT => place.cast::<u16>().write(slot as u16),
            INT => place.cast::<u32>().write(slot as u32),
            FLOAT => place.cast::<f32>().write(f64::from_bits(slot) as f32),
            DOUBLE => place.cast::<f64>().write(f64::from_bits(slot)),
            VOID => {}
            _ => place.cast::<u64>().write(slot),
        }
    }
}

/// Read a value of type `code` at `place` as a slot: sign-extended for a
/// signed integer, zero-extended for an unsigned one or a bool, an `f64`'s
/// bits for a floating-point value.
///
/// # Safety
///
/// `place` is valid for a read of the type's size and aligned for it.
unsafe fn load(code: i64, place: *const c_void) -> u64 {
    let unsigned = code < 0;
    // SAFETY: the caller's contract, for each width.
    unsafe {
        match code.abs() {
            CHAR if unsigned => u64::from(place.cast::<u8>().read()),
            CHAR => place.cast::<i8>().read() as i64 as u64,
            BOOL => u64::from(place.cast::<u8>().read()),
            SHORT if unsigned => u64::from(place.cast::<u16>().read()),
            SHORT => place.cast::<i16>().read() as i64 as u64,
            INT if unsigned => u64::from(place.cast::<u32>().read()),
            INT => place.cast::<i32>().read() as i64 as u64,
            FLOAT => f64::from(place.cast::<f32>().read()).to_bits(),
            DOUBLE => place.cast::<f64>().read().to_bits(),
            VOID => 0,
            _ => place.cast::<u64>().read(),
        }
    }
}

/// Read a call's return value. libffi widens an integral result narrower
/// than a register to a whole `ffi_arg`, so those are read at that width.
///
/// # Safety
///
/// `place` holds what `ffi_call` wrote for a function returning `code`.
unsafe fn load_return(code: i64, place: *const c_void) -> u64 {
    // SAFETY: the caller's contract; the widened value is an `ffi_arg`.
    unsafe {
        match code.abs() {
            CHAR | SHORT | INT | BOOL => {
                // `ffi_arg` is 64 bits wherever Spinel builds.
                #[allow(clippy::useless_conversion)]
                let wide = u64::from(place.cast::<low::ffi_arg>().read());
                let mut narrow = [0u64; 1];
                store(code, wide, narrow.as_mut_ptr().cast());
                load(code, narrow.as_ptr().cast())
            }
            _ => load(code, place),
        }
    }
}

/// Write a closure's return value where libffi reads it, widened to an
/// `ffi_arg` when narrower, as libffi requires of a closure.
///
/// # Safety
///
/// `place` is libffi's return buffer for a closure returning `code`.
unsafe fn store_return(code: i64, slot: u64, place: *mut c_void) {
    // SAFETY: the caller's contract; the buffer holds at least an `ffi_arg`.
    unsafe {
        match code.abs() {
            CHAR | SHORT | INT | BOOL => {
                let mut narrow = [0u64; 1];
                store(code, slot, narrow.as_mut_ptr().cast());
                let widened = load(code, narrow.as_ptr().cast());
                // `ffi_arg` is a register: 64 bits on every platform Spinel
                // builds for, where this is no cast at all.
                #[allow(clippy::unnecessary_cast)]
                place.cast::<low::ffi_arg>().write(widened as low::ffi_arg);
            }
            _ => store(code, slot, place),
        }
    }
}

/// What a call came to: its return value as a slot and the `errno` it left,
/// or a closure it called, waiting for Ruby to answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done {
        value: u64,
        errno: i32,
    },
    Callback {
        call: u64,
        closure: u32,
        args: Vec<u64>,
    },
}

/// What a suspended call's trampoline hands the primitive: which closure C
/// called, and its arguments as slots.
struct Request {
    closure: u32,
    args: Vec<u64>,
}

type CallYielder = Yielder<u64, Request>;
type CallCoroutine = Coroutine<u64, Request, (u64, i32), DefaultStack>;

/// The coroutine of each call running C right now, innermost last, with the
/// OS thread it runs on: what a trampoline suspends. Only the innermost can
/// be running C, since an outer one is suspended in a trampoline of its own
/// (which took itself off) or is waiting on the inner call's primitive.
#[derive(Default)]
struct Running(Mutex<Vec<(usize, usize)>>);

impl Running {
    fn push(&self, yielder: usize) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((yielder, thread_id()));
    }

    /// The innermost call's yielder, if it runs on this OS thread.
    fn pop_mine(&self) -> Option<usize> {
        let mut running = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match running.last() {
            Some(&(yielder, thread)) if thread == thread_id() => {
                running.pop();
                Some(yielder)
            }
            _ => None,
        }
    }
}

fn thread_id() -> usize {
    // SAFETY: `pthread_self` has no preconditions.
    unsafe { libc::pthread_self() as usize }
}

/// What a call needs while it runs: its CIF, its arguments in the C types
/// the CIF says and the pointers to them libffi reads, and where libffi
/// writes the answer. Boxed, so the coroutine can point at it while the
/// record moves, and owned outside the coroutine, so an abandoned call's
/// stack holds nothing to drop.
struct CallData {
    cif: Cif,
    ret: i64,
    func: usize,
    values: Vec<u64>,
    pointers: Vec<*mut c_void>,
    answer: [u64; 2],
}

impl CallData {
    fn new(
        func: usize,
        abi: i64,
        types: &[i64],
        fixed: usize,
        ret: i64,
        slots: &[u64],
    ) -> Result<Box<CallData>, String> {
        let unknown = |code| format!("unknown type {code}");
        let arg_types = types
            .iter()
            .map(|&code| ffi_type(code))
            .collect::<Result<Vec<_>, _>>()
            .map_err(unknown)?;
        let result = ffi_type(ret).map_err(unknown)?;
        let abi = low::ffi_abi::try_from(abi).map_err(|_| "error creating CIF 2".to_owned())?;
        let cif = if fixed < types.len() {
            Cif::try_new_variadic_with_abi(arg_types, fixed, result, abi)
        } else {
            Cif::try_new_with_abi(arg_types, result, abi)
        }
        .map_err(|error| format!("error creating CIF {}", status(&error)))?;
        let mut data = Box::new(CallData {
            cif,
            ret,
            func,
            values: vec![0; types.len()],
            pointers: Vec::with_capacity(types.len()),
            answer: [0; 2],
        });
        for (index, &code) in types.iter().enumerate() {
            let slot = slots.get(index).copied().unwrap_or(0);
            let place: *mut c_void = (&raw mut data.values[index]).cast();
            // SAFETY: each value is a `u64`, room and alignment for any of
            // the types a slot stands for.
            unsafe { store(code, slot, place) };
            data.pointers.push(place);
        }
        Ok(data)
    }

    /// Make the call, answering its return value and the `errno` it left.
    ///
    /// # Safety
    ///
    /// `func` is a function of the CIF's signature; what the arguments point
    /// at is the program's to get right, as it is in C.
    unsafe fn call(&mut self) -> (u64, i32) {
        // SAFETY: the caller's contract. `func` was checked non-null when
        // the call was made.
        unsafe {
            let func: unsafe extern "C" fn() =
                std::mem::transmute::<usize, unsafe extern "C" fn()>(self.func);
            libffi::raw::ffi_call(
                self.cif.as_raw_ptr(),
                Some(func),
                self.answer.as_mut_ptr().cast(),
                self.pointers.as_mut_ptr(),
            );
        }
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        // SAFETY: libffi wrote the return value for `ret` there.
        let value = unsafe { load_return(self.ret, self.answer.as_ptr().cast()) };
        (value, errno)
    }
}

/// A call suspended in a trampoline, waiting for Ruby's answer.
struct Suspended {
    coroutine: CallCoroutine,
    data: Box<CallData>,
    thread: usize,
}

/// A closure: the libffi closure C calls, the CIF it was made with, and the
/// types its trampoline converts by. Boxed, since the trampoline is handed
/// its address.
struct ClosureRecord {
    id: u32,
    cif: Cif,
    ret: i64,
    args: Vec<i64>,
    closure: *mut low::ffi_closure,
    code: usize,
    running: *const Running,
}

/// The trampoline every closure runs: convert C's arguments to slots,
/// suspend the call's stack with them, and return Ruby's answer to C.
///
/// A closure called where no Ruby can answer it — on an OS thread that is not
/// running this heap's call, or outside any call — ends the process, as
/// CRuby's `rb_thread_call_with_gvl` does when a thread Ruby does not know
/// calls it.
unsafe extern "C" fn trampoline(
    _cif: *mut low::ffi_cif,
    result: *mut c_void,
    args: *mut *mut c_void,
    data: *mut c_void,
) {
    // SAFETY: `data` is the `ClosureRecord` this closure was prepared with,
    // which lives until the closure is freed; `args` has one pointer per
    // argument type, each at a value of that type.
    unsafe {
        let record = &*data.cast::<ClosureRecord>();
        let slots = record
            .args
            .iter()
            .enumerate()
            .map(|(index, &code)| load(code, (*args.add(index)).cast_const()))
            .collect();
        let running = &*record.running;
        let Some(yielder) = running.pop_mine() else {
            eprintln!("[BUG] a Fiddle::Closure was called from a thread that is not running Ruby");
            std::process::exit(1);
        };
        let yielder = &*(yielder as *const CallYielder);
        let answer = yielder.suspend(Request {
            closure: record.id,
            args: slots,
        });
        running.push(std::ptr::from_ref(yielder) as usize);
        store_return(record.ret, answer, result);
    }
}

/// One heap's FFI state: its live closures and its suspended calls.
pub struct Ffi {
    closures: Vec<Option<Box<ClosureRecord>>>,
    live: usize,
    calls: HashMap<u64, Suspended>,
    next_call: u64,
    spare: Option<DefaultStack>,
    running: Arc<Running>,
}

impl Default for Ffi {
    fn default() -> Ffi {
        Ffi {
            closures: Vec::new(),
            live: 0,
            calls: HashMap::new(),
            next_call: 0,
            spare: None,
            running: Arc::new(Running::default()),
        }
    }
}

impl std::fmt::Debug for Ffi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ffi")
            .field("closures", &self.live)
            .field("suspended", &self.calls.len())
            .finish()
    }
}

impl Ffi {
    /// Call `func` with `slots` as arguments of `types` — the first `fixed`
    /// of them fixed and the rest variadic — for a value of type `ret`.
    ///
    /// # Safety
    ///
    /// `func` is the address of a function of that signature, and whatever
    /// the arguments point at is what the function expects: the program's to
    /// get right, as it is in C. An error is a CIF libffi would not prepare.
    pub unsafe fn call(
        &mut self,
        func: usize,
        abi: i64,
        types: &[i64],
        fixed: usize,
        ret: i64,
        slots: &[u64],
    ) -> Result<Outcome, String> {
        if func == 0 {
            return Err("NULL function pointer".to_owned());
        }
        let mut data = CallData::new(func, abi, types, fixed, ret, slots)?;
        if self.live == 0 {
            // No closure, so nothing C calls can ask for Ruby: no stack of
            // its own needed.
            // SAFETY: the caller's contract.
            let (value, errno) = unsafe { data.call() };
            return Ok(Outcome::Done { value, errno });
        }
        let stack = match self.spare.take() {
            Some(stack) => stack,
            None => DefaultStack::new(STACK_SIZE)
                .map_err(|error| format!("no stack for the call: {error}"))?,
        };
        let running: *const Running = Arc::as_ptr(&self.running);
        let call_data: *mut CallData = &raw mut *data;
        let running_at = running as usize;
        let data_at = call_data as usize;
        let coroutine = CallCoroutine::with_stack(stack, move |yielder: &CallYielder, _| {
            let running = running_at as *const Running;
            // SAFETY: `running` is this heap's, which outlives every call it
            // makes, and the call's data outlives the coroutine (`Suspended`
            // drops the coroutine first). Nothing on this stack needs
            // dropping, so abandoning it is sound.
            unsafe {
                (*running).push(std::ptr::from_ref(yielder) as usize);
                let answer = (*(data_at as *mut CallData)).call();
                (*running).pop_mine();
                answer
            }
        });
        self.next_call += 1;
        let suspended = Suspended {
            coroutine,
            data,
            thread: thread_id(),
        };
        Ok(self.drive(self.next_call, suspended, 0))
    }

    /// Hand `answer` to the closure call `call` is suspended in, and run the
    /// call on until it finishes or calls a closure again.
    pub fn resume(&mut self, call: u64, answer: u64) -> Result<Outcome, String> {
        let suspended = self
            .calls
            .remove(&call)
            .ok_or_else(|| "no Fiddle call is waiting for that callback".to_owned())?;
        if suspended.thread != thread_id() {
            self.calls.insert(call, suspended);
            return Err("a Fiddle call resumed on another thread".to_owned());
        }
        Ok(self.drive(call, suspended, answer))
    }

    fn drive(&mut self, id: u64, mut suspended: Suspended, input: u64) -> Outcome {
        match suspended.coroutine.resume(input) {
            CoroutineResult::Yield(Request { closure, args }) => {
                self.calls.insert(id, suspended);
                Outcome::Callback {
                    call: id,
                    closure,
                    args,
                }
            }
            CoroutineResult::Return((value, errno)) => {
                self.reclaim(suspended);
                Outcome::Done { value, errno }
            }
        }
    }

    /// Give up on a suspended call: the closure's block left by an
    /// exception, `break` or `throw`, so C never sees an answer and the rest
    /// of the C call never runs.
    pub fn abandon(&mut self, call: u64) {
        if let Some(mut suspended) = self.calls.remove(&call) {
            // SAFETY: the coroutine is suspended in a trampoline, and nothing
            // on its stack needs dropping: the call's data is owned outside
            // it, and the trampoline moved its arguments out before it
            // suspended.
            unsafe { suspended.coroutine.force_reset() };
            self.reclaim(suspended);
        }
    }

    fn reclaim(&mut self, suspended: Suspended) {
        let Suspended {
            coroutine, data, ..
        } = suspended;
        let stack = coroutine.into_stack();
        if self.spare.is_none() {
            self.spare = Some(stack);
        }
        drop(data);
    }

    /// Make a closure for a function of `args` returning `ret`: its id and
    /// the code address C calls.
    pub fn closure(&mut self, ret: i64, args: &[i64], abi: i64) -> Result<(u32, usize), String> {
        let unknown = |code| format!("unknown type {code}");
        let arg_types = args
            .iter()
            .map(|&code| ffi_type(code))
            .collect::<Result<Vec<_>, _>>()
            .map_err(unknown)?;
        let result = ffi_type(ret).map_err(unknown)?;
        let abi = low::ffi_abi::try_from(abi).map_err(|_| "error prepping CIF 2".to_owned())?;
        let cif = Cif::try_new_with_abi(arg_types, result, abi)
            .map_err(|error| format!("error prepping CIF {}", status(&error)))?;
        let (closure, code) =
            low::try_closure_alloc().ok_or_else(|| "error allocating closure".to_owned())?;
        let id = match self.closures.iter().position(Option::is_none) {
            Some(free) => free,
            None => {
                self.closures.push(None);
                self.closures.len() - 1
            }
        };
        let id = u32::try_from(id).map_err(|_| "too many closures".to_owned())?;
        let mut record = Box::new(ClosureRecord {
            id,
            cif,
            ret,
            args: args.to_vec(),
            closure,
            code: code.as_ptr() as usize,
            running: Arc::as_ptr(&self.running),
        });
        let data: *mut ClosureRecord = &raw mut *record;
        // SAFETY: the closure was just allocated, the CIF lives in the
        // record as long as the closure does, and so does the record the
        // trampoline is handed.
        let status = unsafe {
            libffi::raw::ffi_prep_closure_loc(
                closure,
                record.cif.as_raw_ptr(),
                Some(trampoline),
                data.cast(),
                code.as_mut_ptr(),
            )
        };
        if status != libffi::raw::ffi_status_FFI_OK {
            // SAFETY: allocated above and never handed out.
            unsafe { low::closure_free(closure) };
            return Err(format!("error prepping closure {status}"));
        }
        let code = record.code;
        self.closures[id as usize] = Some(record);
        self.live += 1;
        Ok((id, code))
    }

    /// Free closure `id`. Its code address must not be called again.
    pub fn free_closure(&mut self, id: u32) {
        if let Some(record) = self.closures.get_mut(id as usize).and_then(Option::take) {
            // SAFETY: allocated by `closure`, freed once, as `take` makes sure.
            unsafe { low::closure_free(record.closure) };
            self.live -= 1;
        }
    }
}

impl Drop for Ffi {
    fn drop(&mut self) {
        let calls: Vec<u64> = self.calls.keys().copied().collect();
        for call in calls {
            self.abandon(call);
        }
        for id in 0..self.closures.len() {
            self.free_closure(id as u32);
        }
    }
}

/// `dlopen(3)` `path`, or the program itself for `None`: the handle, or
/// `dlerror`'s message.
pub fn dlopen(path: Option<&[u8]>, flags: i32) -> Result<usize, String> {
    let path = match path {
        Some(path) => Some(CString::new(path).map_err(|_| "string contains null byte".to_owned())?),
        None => None,
    };
    let pointer = path.as_ref().map_or(std::ptr::null(), |path| path.as_ptr());
    // SAFETY: a valid C string or null. `dlerror` is read straight after, on
    // this thread.
    let handle = unsafe { libc::dlopen(pointer, flags) };
    if handle.is_null() {
        Err(dlerror().unwrap_or_else(|| "unknown error".to_owned()))
    } else {
        Ok(handle as usize)
    }
}

/// `dlsym(3)` `name` in `handle`: the address, or `None` for no such symbol.
pub fn dlsym(handle: usize, name: &[u8]) -> Result<Option<usize>, String> {
    let name = CString::new(name).map_err(|_| "string contains null byte".to_owned())?;
    // SAFETY: clearing the error, then a lookup in a handle `dlopen` gave or
    // one of the pseudo-handles; a wrong handle is the program's, as in C.
    unsafe {
        libc::dlerror();
        let address = libc::dlsym(handle as *mut c_void, name.as_ptr());
        let failed = !libc::dlerror().is_null();
        Ok(if address.is_null() || failed {
            None
        } else {
            Some(address as usize)
        })
    }
}

/// `dlclose(3)` `handle`: 0, or `dlerror`'s message.
pub fn dlclose(handle: usize) -> Result<(), String> {
    // SAFETY: a handle `dlopen` gave, closed once, as the Ruby side makes sure.
    let status = unsafe { libc::dlclose(handle as *mut c_void) };
    if status == 0 {
        Ok(())
    } else {
        Err(dlerror().unwrap_or_else(|| "could not close handle".to_owned()))
    }
}

fn dlerror() -> Option<String> {
    // SAFETY: `dlerror` answers null or a C string valid until the next call.
    unsafe {
        let message = libc::dlerror();
        (!message.is_null()).then(|| CStr::from_ptr(message).to_string_lossy().into_owned())
    }
}

/// The pseudo-handles: `RTLD_DEFAULT` and `RTLD_NEXT`.
#[must_use]
pub fn pseudo_handles() -> (usize, usize) {
    (libc::RTLD_DEFAULT as usize, libc::RTLD_NEXT as usize)
}

/// A copy of the `len` bytes at `address`.
///
/// # Safety
///
/// They are readable: the program's to get right, as it is in C.
#[must_use]
pub unsafe fn read(address: usize, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    if len > 0 {
        // SAFETY: the caller's contract; the copy does not overlap `bytes`.
        unsafe { std::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), len) };
    }
    bytes
}

/// Copy `bytes` to `address`.
///
/// # Safety
///
/// That many bytes there are writable: the program's to get right.
pub unsafe fn write(address: usize, bytes: &[u8]) {
    if !bytes.is_empty() {
        // SAFETY: the caller's contract.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len()) };
    }
}

/// `strlen(3)` of the C string at `address`.
///
/// # Safety
///
/// A NUL-terminated string is there.
#[must_use]
pub unsafe fn strlen(address: usize) -> usize {
    // SAFETY: the caller's contract.
    unsafe { libc::strlen(address as *const libc::c_char) }
}

/// `calloc(3)` `size` zeroed bytes: the address, 0 when there is no memory.
#[must_use]
pub fn malloc(size: usize) -> usize {
    // SAFETY: no preconditions; one byte at least, so success is never null.
    unsafe { libc::calloc(1, size.max(1)) as usize }
}

/// `realloc(3)` the block at `address` to `size` bytes.
///
/// # Safety
///
/// `address` is 0 or a block `malloc` or `realloc` gave and not yet freed.
#[must_use]
pub unsafe fn realloc(address: usize, size: usize) -> usize {
    // SAFETY: the caller's contract.
    unsafe { libc::realloc(address as *mut c_void, size.max(1)) as usize }
}

/// `free(3)` the block at `address`.
///
/// # Safety
///
/// As [`realloc`]'s, and the block is not used again.
pub unsafe fn free(address: usize) {
    // SAFETY: the caller's contract.
    unsafe { libc::free(address as *mut c_void) }
}

/// The address of the C library's `free`, which `Fiddle::RUBY_FREE` is:
/// what a `Fiddle::Pointer` frees memory `Fiddle.malloc` gave with.
#[must_use]
pub fn free_function() -> usize {
    libc::free as *const () as usize
}

/// libffi's default calling convention, `Fiddle::Function::DEFAULT`.
#[must_use]
pub fn default_abi() -> i64 {
    i64::from(low::ffi_abi_FFI_DEFAULT_ABI)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str) -> usize {
        let (default, _) = pseudo_handles();
        dlsym(default, name.as_bytes())
            .expect("a name")
            .unwrap_or_else(|| panic!("{name} is in the C library"))
    }

    fn done(outcome: Outcome) -> u64 {
        match outcome {
            Outcome::Done { value, .. } => value,
            other => panic!("expected the call to finish: {other:?}"),
        }
    }

    #[test]
    fn calls_a_c_function_with_integers_and_doubles() {
        let mut ffi = Ffi::default();
        let abs = symbol("labs");
        let answer = unsafe { ffi.call(abs, default_abi(), &[LONG], 1, LONG, &[(-42i64) as u64]) };
        assert_eq!(done(answer.expect("a call")) as i64, 42);
        let pow = symbol("pow");
        let slots = [2.0f64.to_bits(), 10.0f64.to_bits()];
        let answer = unsafe { ffi.call(pow, default_abi(), &[DOUBLE, DOUBLE], 2, DOUBLE, &slots) };
        assert_eq!(f64::from_bits(done(answer.expect("a call"))), 1024.0);
    }

    #[test]
    fn narrow_returns_are_sign_or_zero_extended() {
        let mut ffi = Ffi::default();
        let abs = symbol("abs");
        let answer = unsafe { ffi.call(abs, default_abi(), &[INT], 1, INT, &[(-7i64) as u64]) };
        assert_eq!(done(answer.expect("a call")) as i64, 7);
        // `abs(INT_MIN)` is INT_MIN, read back as a negative int.
        let answer = unsafe {
            ffi.call(
                abs,
                default_abi(),
                &[INT],
                1,
                -INT,
                &[i64::from(i32::MIN) as u64],
            )
        };
        assert_eq!(done(answer.expect("a call")), u64::from(i32::MIN as u32));
    }

    #[test]
    fn a_closure_called_from_c_suspends_the_call_until_ruby_answers() {
        let mut ffi = Ffi::default();
        let (id, code) = ffi
            .closure(INT, &[VOIDP, VOIDP], default_abi())
            .expect("a closure");
        let mut numbers: Vec<i32> = vec![5, 3, 9, 1, 7];
        let qsort = symbol("qsort");
        let slots = [numbers.as_mut_ptr() as u64, 5, 4, code as u64];
        let mut outcome = unsafe {
            ffi.call(
                qsort,
                default_abi(),
                &[VOIDP, -LONG, -LONG, VOIDP],
                4,
                VOID,
                &slots,
            )
        }
        .expect("a call");
        let mut compares = 0;
        while let Outcome::Callback {
            call,
            closure,
            args,
        } = outcome
        {
            assert_eq!(closure, id);
            compares += 1;
            let (a, b) = unsafe { (*(args[0] as *const i32), *(args[1] as *const i32)) };
            outcome = ffi
                .resume(call, i64::from((a - b).signum()) as u64)
                .expect("resumed");
        }
        assert!(compares > 0);
        assert_eq!(numbers, [1, 3, 5, 7, 9]);
        ffi.free_closure(id);
    }

    #[test]
    fn an_abandoned_call_leaves_the_heap_able_to_call_again() {
        let mut ffi = Ffi::default();
        let (id, code) = ffi
            .closure(INT, &[VOIDP, VOIDP], default_abi())
            .expect("a closure");
        let qsort = symbol("qsort");
        let mut numbers: Vec<i32> = vec![2, 1];
        let slots = [numbers.as_mut_ptr() as u64, 2, 4, code as u64];
        let outcome = unsafe {
            ffi.call(
                qsort,
                default_abi(),
                &[VOIDP, -LONG, -LONG, VOIDP],
                4,
                VOID,
                &slots,
            )
        }
        .expect("a call");
        let Outcome::Callback { call, .. } = outcome else {
            panic!("qsort compares two numbers");
        };
        ffi.abandon(call);
        assert!(ffi.resume(call, 0).is_err(), "an abandoned call is gone");
        let abs = symbol("abs");
        let answer = unsafe { ffi.call(abs, default_abi(), &[INT], 1, INT, &[(-3i64) as u64]) };
        assert_eq!(done(answer.expect("a call")) as i64, 3);
        ffi.free_closure(id);
    }

    #[test]
    fn layouts_follow_the_c_abi() {
        assert_eq!(layout(VOIDP), Some((8, 8)));
        assert_eq!(layout(INT), Some((4, 4)));
        assert_eq!(layout(-SHORT), Some((2, 2)));
        assert_eq!(layout(DOUBLE), Some((8, 8)));
        assert_eq!(layout(BOOL), Some((1, 1)));
        assert_eq!(layout(VOID), None);
    }
}
