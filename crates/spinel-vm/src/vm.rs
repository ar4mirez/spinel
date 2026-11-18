//! What the Ractors of one program share: its classes and modules, the method
//! bodies their tables point at, and the class lock (#117).
//!
//! `docs/engine.md` ("Ractors and threads") makes a class a shared object:
//! one method table, one set of constants, one serial, which every Ractor
//! reads and the class lock serialises writes to. So the table is not a
//! heap's. It belongs to a [`Vm`], which the first heap of a program makes and
//! every Ractor's heap after it joins. It is not process-global: each
//! [`Heap::new`](crate::Heap::new) is a program of its own, which is what the
//! tests that make one heap per case rely on. `CLAUDE.md`'s rule against
//! global VM state holds without an exception in `shared/`.
//!
//! What stays per heap is everything a heap allocates. A class object is a
//! heap cell, and an instance's header points at a class object of its own
//! heap, so each heap keeps one object per class, indexed by [`ClassId`]
//! (`Heap::class_objects`), and the table is keyed by the id rather than by
//! any heap's object. The core library's hidden instance variables, the
//! `@__name__` spelling, live on those per-heap objects, so each Ractor has
//! its own (`Random`'s default generator, `Kernel`'s recursion guards,
//! `Encoding`'s defaults).
//!
//! # The class lock
//!
//! A read of the table is on every send: the inline cache compares a class's
//! serial, and a miss walks the chain. A lock taken per read would cost every
//! Ractor, and would cost a program that never makes one too. So the lock is
//! the other way round, as CRuby's VM barrier is. A running Ractor *holds* the
//! table, and reading it then takes nothing. A write waits until every other
//! Ractor has let its hold go at a safe point, writes alone, and lets the
//! others back at its own next safe point. Writes are rare after boot (a
//! definition, an `include`, a constant assignment, a method-cache fill), so
//! that is cheap where it matters.
//!
//! While a program has only ever had one Ractor, none of that runs. The lock's
//! `shared` flag is false, and a write pays one load of it and a branch.
//!
//! [`ClassId`]: crate::class::ClassId

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

use crate::class::Classes;
use crate::method::Definitions;

/// One program's classes, method bodies and class lock. See the module docs.
pub(crate) struct Vm {
    classes: UnsafeCell<Classes>,
    definitions: UnsafeCell<Definitions>,
    lock: ClassLock,
}

// SAFETY: the two tables are reached only through `Heap`'s accessors. A heap
// reads them while it holds the class lock's hold, and writes them only while
// it is the lock's one writer (`ClassLock::write`), with every other holder
// parked at a safe point. While `shared` is false there is one heap, and its
// own `&`/`&mut` borrows are what keep reads from writes.
unsafe impl Sync for Vm {}
// SAFETY: as above. Nothing in either table is tied to the OS thread that made
// it, and a heap already goes from one OS thread to another (`threads.rs`).
unsafe impl Send for Vm {}

impl Vm {
    pub(crate) fn new() -> Vm {
        Vm {
            classes: UnsafeCell::new(Classes::new()),
            definitions: UnsafeCell::new(Definitions::new()),
            lock: ClassLock::new(),
        }
    }

    /// The table, to read.
    ///
    /// # Safety
    ///
    /// The caller is a heap of this program that holds the class lock (every
    /// running heap does, see [`ClassLock`]) and has no `&mut` to the table
    /// alive. `Heap::classes` is the one caller.
    #[inline]
    pub(crate) unsafe fn classes(&self) -> &Classes {
        // SAFETY: per the contract, no writer is active.
        unsafe { &*self.classes.get() }
    }

    /// The table, to write.
    ///
    /// # Safety
    ///
    /// The caller has just made itself the writer with [`ClassLock::write`],
    /// or the program has one heap. It must hold no other borrow of the table.
    /// `Heap::classes_mut` is the one caller.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn classes_mut(&self) -> &mut Classes {
        // SAFETY: per the contract, nothing else reads or writes it now.
        unsafe { &mut *self.classes.get() }
    }

    /// The method bodies, to read. The same contract as [`Vm::classes`].
    #[inline]
    pub(crate) unsafe fn definitions(&self) -> &Definitions {
        // SAFETY: as `classes`.
        unsafe { &*self.definitions.get() }
    }

    /// The method bodies, to write. The same contract as [`Vm::classes_mut`].
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub(crate) unsafe fn definitions_mut(&self) -> &mut Definitions {
        // SAFETY: as `classes_mut`.
        unsafe { &mut *self.definitions.get() }
    }

    pub(crate) fn lock(&self) -> &ClassLock {
        &self.lock
    }
}

/// The class lock. Every heap of the program that is running holds it, and
/// reads the tables freely while it does. A heap about to write makes itself
/// the writer: it lets its own hold go and waits for every other to let
/// theirs go at a safe point. It writes alone until its own next safe point.
///
/// A heap's part of the state is a `bool` it keeps itself ([`Holder`]): whether
/// it is the writer now. The rest is here, under one mutex. A write and a safe
/// point are the only calls that touch it, and each is one relaxed-to-acquire
/// load and a branch while `shared` is false.
pub(crate) struct ClassLock {
    shared: AtomicBool,
    state: Mutex<LockState>,
    turn: Condvar,
}

#[derive(Default)]
struct LockState {
    /// Heaps holding the lock to read: running, and not parked at a safe
    /// point for a writer.
    holding: usize,
    /// Whether a heap is writing now.
    writing: bool,
    /// Heaps waiting to write. While any is, a heap coming back from a safe
    /// point waits, so writers are not starved by readers that never stop.
    waiting: usize,
}

/// A heap's part of the class lock: whether it is the writer now.
#[derive(Debug, Default)]
pub(crate) struct Holder {
    writing: bool,
}

impl ClassLock {
    fn new() -> ClassLock {
        ClassLock {
            shared: AtomicBool::new(false),
            // The heap that makes the program holds it from the start.
            state: Mutex::new(LockState {
                holding: 1,
                ..LockState::default()
            }),
            turn: Condvar::new(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, LockState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new heap joins the program, holding the lock. Its creator holds the
    /// lock to read while it calls this, and is not writing, so no writer is
    /// active: the new heap starts as a reader among readers.
    pub(crate) fn join(&self, creator: &Holder) {
        assert!(!creator.writing, "a heap joins the program between writes");
        let mut state = self.state();
        debug_assert!(!state.writing && state.holding > 0);
        state.holding += 1;
        self.shared.store(true, Ordering::Release);
    }

    /// Make `me` the writer, unless it is already or the program has one heap.
    /// Called before every write to the tables.
    #[inline]
    pub(crate) fn write(&self, me: &mut Holder) {
        if me.writing || !self.shared.load(Ordering::Acquire) {
            return;
        }
        self.write_slow(me);
    }

    #[cold]
    fn write_slow(&self, me: &mut Holder) {
        let mut state = self.state();
        state.holding -= 1;
        state.waiting += 1;
        self.turn.notify_all();
        while state.writing || state.holding > 0 {
            state = self
                .turn
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.waiting -= 1;
        state.writing = true;
        me.writing = true;
    }

    /// A safe point: `me` holds no borrow of the tables. A writer stops
    /// writing and goes back to reading; a reader that a writer waits for
    /// parks until it is done. One load and a branch while the program has one
    /// heap. The interpreter's turn check calls it.
    #[inline]
    pub(crate) fn safe_point(&self, me: &mut Holder) {
        if self.shared.load(Ordering::Acquire) {
            self.safe_point_slow(me);
        }
    }

    #[cold]
    fn safe_point_slow(&self, me: &mut Holder) {
        let mut state = self.state();
        if me.writing {
            me.writing = false;
            state.writing = false;
            state.holding += 1;
            self.turn.notify_all();
        }
        if state.writing || state.waiting > 0 {
            state.holding -= 1;
            self.turn.notify_all();
            while state.writing || state.waiting > 0 {
                state = self
                    .turn
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            state.holding += 1;
        }
    }

    /// `me` stops using the tables for a while: it is about to wait, or it
    /// is done. A writer stops writing. Every call is matched by
    /// [`ClassLock::reacquire`], except the last of a heap that is going away.
    pub(crate) fn release(&self, me: &mut Holder) {
        if !self.shared.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state();
        if me.writing {
            me.writing = false;
            state.writing = false;
        } else {
            state.holding -= 1;
        }
        self.turn.notify_all();
    }

    /// `me` is back from what [`ClassLock::release`] was before, and reads
    /// again once no writer is writing or waiting.
    pub(crate) fn reacquire(&self, me: &mut Holder) {
        debug_assert!(!me.writing);
        if !self.shared.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state();
        while state.writing || state.waiting > 0 {
            state = self
                .turn
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.holding += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    /// The protocol on its own: two readers that read a counter only while
    /// holding, and a writer that bumps it in two halves that must never be
    /// seen apart.
    #[test]
    fn a_writer_is_alone_and_readers_never_see_half_a_write() {
        let lock = Arc::new(ClassLock::new());
        // The creator joins the others in; each joiner holds from the start.
        let creator = Holder::default();
        lock.join(&creator);
        lock.join(&creator);
        let halves = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let (lock, halves) = (Arc::clone(&lock), Arc::clone(&halves));
                std::thread::spawn(move || {
                    let mut me = Holder::default();
                    for _ in 0..20_000 {
                        let a = halves[0].load(Ordering::Relaxed);
                        let b = halves[1].load(Ordering::Relaxed);
                        assert_eq!(a, b, "a reader saw half a write");
                        lock.safe_point(&mut me);
                    }
                    lock.release(&mut me);
                })
            })
            .collect();
        let mut me = Holder::default();
        for _ in 0..2_000 {
            lock.write(&mut me);
            halves[0].fetch_add(1, Ordering::Relaxed);
            std::thread::yield_now();
            halves[1].fetch_add(1, Ordering::Relaxed);
            lock.safe_point(&mut me);
        }
        lock.release(&mut me);
        for reader in readers {
            reader.join().expect("the reader finishes");
        }
        assert_eq!(halves[0].load(Ordering::Relaxed), 2_000);
    }

    #[test]
    fn one_heap_never_takes_the_mutex() {
        let lock = ClassLock::new();
        let mut me = Holder::default();
        lock.write(&mut me);
        assert!(!me.writing, "a program with one heap has no writer to be");
        lock.safe_point(&mut me);
        lock.release(&mut me);
        lock.reacquire(&mut me);
        assert_eq!(lock.state().holding, 1);
    }
}
