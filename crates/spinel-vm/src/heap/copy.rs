//! Copying an object graph out of one heap and into another (#117).
//!
//! A Ractor's heap holds no pointer into another's, so whatever crosses
//! between two Ractors crosses as a copy: a message, the arguments of
//! `Ractor.new`, its block's value, an exception it raised, and a shareable
//! constant the main Ractor set that another reads (`docs/engine.md`,
//! "Ractors and threads"; decision 2a of #117). [`Heap::export`] reads the
//! graph under a root into a [`Msg`], which holds no heap's pointers and can
//! go to another OS thread; [`HandleScope::import`] builds it again, cell
//! for cell, in the heap that reads it.
//!
//! The copy is of cells, not of Ruby objects: every object here is a header
//! and its slots or bytes, so the same walk copies a `String` and its
//! buffer, an `Array` and its storage, a `Hash`'s pairs, a `Struct`'s values
//! and an object's instance variables, frozen marks included. What a cell
//! cannot carry is per heap and is rebuilt: a class is named by its
//! [`ClassId`] and is each heap's own object for it, a shape by the names
//! it holds, a compiled `Regexp` by the pattern itself. What holds per-heap
//! state that has no such translation — a `Proc`'s frames, a `Thread`'s
//! entry, a `Method` — is refused, as CRuby refuses it, by the classes the
//! caller names.
//!
//! An object whose class is a singleton class is copied as an instance of
//! the class it was made from: the singleton belongs to the object left
//! behind.

use std::collections::HashMap;
use std::sync::Arc;

use super::{
    CHILLED, CHILLED_SYMBOL, FROZEN, HandleScope, Heap, KEYWORDS, Obj, Payload, SIDE_IVARS,
};
use crate::class::{Builtin, ClassId};
use crate::shape::ShapeId;
use crate::value::{SymbolId, Value};

/// The flags a copy keeps. The rest — the mark bit, the side-table bit,
/// the allocation bit — are the heap's own bookkeeping.
const COPIED_FLAGS: u8 = FROZEN | CHILLED | CHILLED_SYMBOL | KEYWORDS;

/// A class whose instances may not be copied, and the exception a copy of
/// one raises, as CRuby 4.0.9 raises it: `allocator undefined for Proc`, a
/// `TypeError`, and `can not copy Enumerator object.`, a `Ractor::Error`.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub class: ClassId,
    /// The exception's class, by name.
    pub error: String,
    pub message: String,
}

/// An object graph out of one heap, holding no pointer into it: what crosses
/// between Ractors. See the module docs.
#[derive(Debug)]
pub struct Msg {
    nodes: Vec<Node>,
    root: Item,
}

// SAFETY: a `Msg` holds immediates, class ids, bytes and compiled patterns,
// and no heap pointer: an immediate is the whole object, and `export` turns
// every heap object into a node.
unsafe impl Send for Msg {}
// SAFETY: as above; nothing in it is written after it is made.
unsafe impl Sync for Msg {}

#[derive(Debug, Clone, Copy)]
enum Item {
    /// An immediate: the word is the object.
    Imm(Value),
    /// A class or module, which every heap has an object of its own for.
    Class(ClassId),
    /// A heap object, by its index among the nodes.
    Node(u32),
    /// An object of the importing heap already: see [`Options::known`].
    Known(Value),
}

#[derive(Debug)]
struct Node {
    /// `None` for a cell Ruby never sees: a `String`'s buffer, an object's
    /// instance-variable storage.
    class: Option<ClassId>,
    flags: u8,
    /// The instance variables' names, in the order the object's shape holds
    /// them; `None` for an object with no shape.
    shape: Option<Vec<SymbolId>>,
    /// Instance-variable storage kept in the heap's side table.
    side: Option<Item>,
    body: Body,
}

#[derive(Debug)]
enum Body {
    Slots(Vec<Item>),
    Bytes(Vec<u8>),
    /// A `Regexp`: its slots, and the compiled pattern its first one indexes
    /// in the heap's table.
    Regexp(Arc<spinel_regex::Regex>, Vec<Item>),
}

/// Why [`Heap::export`] stopped.
#[derive(Debug, Clone)]
pub enum ExportError {
    /// An object of a class the caller refuses to copy.
    Refused(Refusal),
    /// An object that is not frozen, where only shareable ones may go.
    Unshareable,
}

impl Msg {
    /// The message for an immediate, which is the object itself.
    pub fn immediate(value: Value) -> Msg {
        debug_assert!(value.is_immediate());
        Msg {
            nodes: Vec::new(),
            root: Item::Imm(value),
        }
    }
}

/// How [`Heap::export`] copies.
#[derive(Default)]
pub(crate) struct Options<'a> {
    /// The classes not to copy.
    pub refusals: &'a [Refusal],
    /// Refuse any object Ruby sees that is not frozen: CRuby's "shareable",
    /// for what a copy reaches. A core class's own state, the instance
    /// variables spelled `@__name__` (a `Hash`'s pairs, a `Struct`'s
    /// values), is not held to it, though what it holds is: the Arrays it
    /// is built of are its, the objects in them the program's.
    pub shareable_only: bool,
    /// Objects of the exported heap already copied into the importing one,
    /// by address: the copy names them rather than copying them again, so
    /// one object read twice is one object (`HandleScope::pull`).
    pub known: Option<&'a HashMap<usize, Value>>,
}

/// What [`Heap::export`] also answers: the objects each node was copied
/// from, by address, in node order, for [`Options::known`] next time.
pub(crate) type Sources = Vec<usize>;

impl Heap {
    /// The graph under `root`, copied out, as `options` says. Each object
    /// Ruby sees that the copy took is pushed onto `taken`, for a move to
    /// leave behind.
    ///
    /// `self` need not be the heap of the OS thread calling: a heap reads
    /// the main one's constants this way, while that one is parked (see
    /// `HandleScope::pull`). It writes nothing.
    pub(crate) fn export(
        &self,
        root: Value,
        options: &Options<'_>,
        mut taken: Option<&mut Vec<Value>>,
    ) -> Result<(Msg, Sources), ExportError> {
        let mut export = Export {
            heap: self,
            nodes: Vec::new(),
            sources: Vec::new(),
            seen: HashMap::new(),
            work: Vec::new(),
            storage: HashMap::new(),
            known: options.known,
        };
        let root = export.item(root, false);
        while let Some((value, index, internal)) = export.work.pop() {
            let node = export.node(value, internal, options)?;
            // A frozen object is shareable, and stays the sender's: one a
            // frozen literal answers is every use of that literal's.
            if node.class.is_some()
                && node.flags & FROZEN == 0
                && let Some(taken) = taken.as_deref_mut()
            {
                taken.push(value);
            }
            export.nodes[index as usize] = Some(node);
        }
        let nodes = export
            .nodes
            .into_iter()
            .map(|node| node.expect("every node pushed is filled"))
            .collect();
        Ok((Msg { nodes, root }, export.sources))
    }

    /// The class `value` is, when it is a class or module object of this
    /// heap: its `@__id__`, checked against the heap's object for that id.
    /// Reads the cell only, so it serves [`Heap::export`] on a heap another
    /// OS thread owns.
    pub(crate) fn class_id_of_cell(&self, value: Value) -> Option<ClassId> {
        let pointer = value.as_heap()?;
        let object = Obj(pointer.cast());
        // SAFETY: a live object of this heap; nothing writes it meanwhile
        // (see `export`).
        let header = unsafe { &*object.header() };
        if header.payload != Payload::Slots || header.shape == ShapeId::NONE {
            return None;
        }
        let name = crate::shared::symbols::class_id_ivar();
        let index = self.shapes.index_of(header.shape, name)? as usize;
        let storage = self.ivar_storage_of_cell(object, header)?;
        let id = self.cell_slot(storage, index)?.as_fixnum()?;
        let id = ClassId::from_index(u32::try_from(id).ok()?);
        (self.class_objects.get(id.index()) == Some(&value)).then_some(id)
    }

    /// Instance variable `name` of `object`, read from its cells: what
    /// another heap reads of the main one's class objects while it is
    /// parked. `None` when it holds none by that name.
    pub(crate) fn ivar_of_cell(&self, object: Value, name: SymbolId) -> Option<Value> {
        let cell = Obj(object.as_heap()?.cast());
        // SAFETY: a live object of this heap, which nothing writes meanwhile.
        let header = unsafe { &*cell.header() };
        if header.shape == ShapeId::NONE {
            return None;
        }
        let index = self.shapes.index_of(header.shape, name)? as usize;
        let storage = self.ivar_storage_of_cell(cell, header)?;
        self.cell_slot(storage, index)
    }

    /// The storage object behind a shaped object's instance variables.
    fn ivar_storage_of_cell(&self, object: Obj, header: &super::Header) -> Option<Value> {
        if header.flags & SIDE_IVARS != 0 {
            self.side_ivars.get(&(object.0.as_ptr() as usize)).copied()
        } else if header.len > 0 && header.payload == Payload::Slots {
            // SAFETY: slot 0 of a live object of `len > 0` slots.
            Some(unsafe { object.slots().read() })
        } else {
            None
        }
    }

    /// Slot `index` of the Slots cell `value`, or `None`.
    fn cell_slot(&self, value: Value, index: usize) -> Option<Value> {
        let object = Obj(value.as_heap()?.cast());
        // SAFETY: a live object of this heap.
        let header = unsafe { &*object.header() };
        if header.payload != Payload::Slots || index >= header.len as usize {
            return None;
        }
        // SAFETY: within the object's slots.
        Some(unsafe { object.slots().add(index).read() })
    }

    /// A word that changes when the cell at an address is another object:
    /// its class and length, for [`Options::known`] to tell a freed and
    /// reused address from the object it remembers.
    pub(crate) fn fingerprint_of_cell(&self, value: Value) -> u64 {
        let Some(pointer) = value.as_heap() else {
            return 0;
        };
        let object = Obj(pointer.cast());
        // SAFETY: a live object of this heap.
        let header = unsafe { &*object.header() };
        let class = header.class.map_or(0, Value::to_bits);
        class ^ (u64::from(header.len) << 1) ^ (u64::from(header.payload as u8) << 40)
    }
}

impl Heap {
    /// Freeze every object under `root` that Ruby sees, but classes and
    /// modules: `Ractor.make_shareable`. A `Proc` is frozen and not looked
    /// into. A `Hash` is left for the caller to freeze with `Hash#freeze`,
    /// which keeps its codes and table first, and is answered. Allocates
    /// nothing in the heap, so a graph of any size is walked in a loop with
    /// no handle taken.
    ///
    /// `root` and everything under it must be live objects of this heap.
    pub(crate) fn freeze_deep(&mut self, root: Value) -> Vec<Value> {
        let proc_class = self.class_objects.get(Builtin::Proc as usize).copied();
        let hash = ClassId::from_index(Builtin::Hash as u32);
        let mut hashes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut work = vec![root];
        while let Some(value) = work.pop() {
            let Some(pointer) = value.as_heap() else {
                continue;
            };
            if !seen.insert(pointer.as_ptr() as usize) || self.class_id_of_cell(value).is_some() {
                continue;
            }
            let object = Obj(pointer.cast());
            // SAFETY: a live object of this heap; the walk writes only flags.
            let header = unsafe { &mut *object.header() };
            if let Some(class) = header.class {
                // SAFETY: the tables are read under the class lock the heap
                // running this holds.
                let classes = unsafe { self.vm.classes() };
                let is_hash = self
                    .class_id_of_cell(class)
                    .is_some_and(|id| classes.reaches(id, hash));
                if is_hash {
                    if header.flags & FROZEN == 0 {
                        hashes.push(value);
                    }
                } else {
                    header.flags = (header.flags | FROZEN) & !(CHILLED | CHILLED_SYMBOL);
                }
                if Some(class) == proc_class {
                    continue;
                }
            }
            if header.flags & SIDE_IVARS != 0
                && let Some(&storage) = self.side_ivars.get(&(object.0.as_ptr() as usize))
            {
                work.push(storage);
            }
            if header.payload == Payload::Slots {
                for index in 0..header.len as usize {
                    // SAFETY: within the object's slots.
                    work.push(unsafe { object.slots().add(index).read() });
                }
            }
        }
        hashes
    }
}

/// Whether `name` is a core class's own state: `@__name__`.
pub(crate) fn is_hidden(name: SymbolId) -> bool {
    crate::shared::symbols::name(name)
        .is_some_and(|name| name.len() > 5 && name.starts_with("@__") && name.ends_with("__"))
}

struct Export<'a> {
    heap: &'a Heap,
    nodes: Vec<Option<Node>>,
    sources: Sources,
    seen: HashMap<usize, u32>,
    /// Objects to read: the object, its node, and whether it is a core
    /// class's own state (see [`Options::shareable_only`]).
    work: Vec<(Value, u32, bool)>,
    /// The names an instance-variable storage cell holds, by its address,
    /// recorded by the object it belongs to.
    storage: HashMap<usize, Vec<SymbolId>>,
    known: Option<&'a HashMap<usize, Value>>,
}

impl Export<'_> {
    /// `value` as an item, queueing it to be read when it is an object seen
    /// for the first time. A graph of any depth is read in a loop, as the
    /// collector traces one.
    fn item(&mut self, value: Value, internal: bool) -> Item {
        let Some(pointer) = value.as_heap() else {
            return Item::Imm(value);
        };
        if let Some(id) = self.heap.class_id_of_cell(value) {
            return Item::Class(id);
        }
        let address = pointer.as_ptr() as usize;
        if let Some(known) = self.known.and_then(|known| known.get(&address)) {
            return Item::Known(*known);
        }
        if let Some(&index) = self.seen.get(&address) {
            return Item::Node(index);
        }
        let index = u32::try_from(self.nodes.len()).expect("fewer than 2^32 objects");
        self.seen.insert(address, index);
        self.nodes.push(None);
        self.sources.push(address);
        self.work.push((value, index, internal));
        Item::Node(index)
    }

    /// Whether `value` is a cell Ruby never sees.
    fn is_classless(&self, value: Value) -> bool {
        let Some(pointer) = value.as_heap() else {
            return false;
        };
        let object = Obj(pointer.cast());
        // SAFETY: a live object of the exported heap.
        unsafe { (*object.header()).class.is_none() }
    }

    /// Whether `value` is an `Array` of the exported heap, and nothing else.
    fn is_array(&self, value: Value) -> bool {
        let Some(pointer) = value.as_heap() else {
            return false;
        };
        let object = Obj(pointer.cast());
        // SAFETY: a live object of the exported heap.
        let class = unsafe { (*object.header()).class };
        class.is_some()
            && class
                == self
                    .heap
                    .class_objects
                    .get(Builtin::Array as usize)
                    .copied()
    }

    fn node(
        &mut self,
        value: Value,
        internal: bool,
        options: &Options<'_>,
    ) -> Result<Node, ExportError> {
        let heap = self.heap;
        let object = Obj(value.as_heap().expect("queued as an object").cast());
        // SAFETY: a live object of the exported heap, which nothing writes
        // meanwhile (see `Heap::export`).
        let header = unsafe { &*object.header() };
        let class = match header.class {
            None => None,
            Some(class) => {
                let id = heap
                    .class_id_of_cell(class)
                    .expect("an object's class is a class object of its heap");
                // SAFETY: the tables are read under the class lock, which the
                // exporting OS thread holds or has parked every writer under.
                let classes = unsafe { heap.vm.classes() };
                if let Some(refusal) = options
                    .refusals
                    .iter()
                    .find(|r| classes.reaches(id, r.class))
                {
                    return Err(ExportError::Refused(refusal.clone()));
                }
                if options.shareable_only && !internal && header.flags & FROZEN == 0 {
                    return Err(ExportError::Unshareable);
                }
                let mut id = id;
                while classes.is_singleton(id) {
                    match classes.superclass(id) {
                        Some(superclass) => id = superclass,
                        None => break,
                    }
                }
                Some(id)
            }
        };
        let shape = (header.shape != ShapeId::NONE).then(|| heap.shapes.names(header.shape));
        if let Some(names) = &shape
            && let Some(storage) = heap.ivar_storage_of_cell(object, header)
            && let Some(pointer) = storage.as_heap()
        {
            self.storage
                .insert(pointer.as_ptr() as usize, names.clone());
        }
        let side = if header.flags & SIDE_IVARS != 0 {
            heap.side_ivars
                .get(&(object.0.as_ptr() as usize))
                .copied()
                .map(|storage| self.item(storage, internal))
        } else {
            None
        };
        // What this cell's slots hold: the instance variables a shape names,
        // each by its spelling; otherwise what an internal Array holds is
        // internal when it is an Array too, and a classless cell passes on
        // whatever its owner was.
        let names = self.storage.remove(&(object.0.as_ptr() as usize));
        let len = header.len as usize;
        let body = match header.payload {
            Payload::Bytes => {
                // SAFETY: `len` bytes of a live Bytes cell.
                let bytes = unsafe { std::slice::from_raw_parts(object.bytes(), len) };
                Body::Bytes(bytes.to_vec())
            }
            Payload::Slots => {
                let mut slots = Vec::with_capacity(len);
                for index in 0..len {
                    // SAFETY: within the object's slots.
                    let slot = unsafe { object.slots().add(index).read() };
                    let inner = match &names {
                        Some(names) => names.get(index).is_some_and(|&name| is_hidden(name)),
                        None if class.is_none() || self.is_classless(slot) => internal,
                        None => internal && self.is_array(slot),
                    };
                    slots.push(self.item(slot, inner));
                }
                // SAFETY: as for the refusals above.
                let classes = unsafe { heap.vm.classes() };
                let regexp = class.is_some_and(|id| {
                    classes.repr(id) == Some(Builtin::Regexp)
                        && len == crate::regexp::REGEXP_SLOTS as usize
                });
                let compiled = if regexp {
                    // SAFETY: slot `INDEX` of a Regexp, within its slots.
                    let index = unsafe { object.slots().add(crate::regexp::REGEXP_INDEX).read() };
                    index
                        .as_fixnum()
                        .and_then(|i| heap.regexps.get(i as usize))
                        .cloned()
                } else {
                    None
                };
                match compiled {
                    Some(compiled) => Body::Regexp(compiled, slots),
                    None => Body::Slots(slots),
                }
            }
        };
        Ok(Node {
            class,
            flags: header.flags & COPIED_FLAGS,
            shape,
            side,
            body,
        })
    }
}

/// What [`HandleScope::import`] built: the root, and what the caller is
/// left to do in Ruby, which this cannot: rehash each `Hash` (a key hashed
/// by identity has another identity here), then freeze what was frozen.
pub struct Imported {
    pub root: Value,
    pub hashes: Vec<Value>,
    pub frozen: Vec<Value>,
    /// Every object built, in the message's order.
    pub nodes: Vec<Value>,
}

impl<'h> HandleScope<'h> {
    /// Build `msg` in this heap. Every object is allocated first and its
    /// slots written after, so a graph with cycles and shared objects comes
    /// out with the same ones. The objects come out unfrozen: see
    /// [`Imported`]. Every object stays rooted in this scope.
    pub fn import(&mut self, msg: &Msg) -> Imported {
        let scope = self;
        // A class another Ractor defined since this heap last looked.
        scope.catch_up_classes();
        let mut handles = Vec::with_capacity(msg.nodes.len());
        for node in &msg.nodes {
            let class = node.class.map(|id| {
                let object = scope.class_object(id);
                scope.root(object)
            });
            let (payload, len) = match &node.body {
                Body::Slots(slots) | Body::Regexp(_, slots) => (Payload::Slots, slots.len()),
                Body::Bytes(bytes) => (Payload::Bytes, bytes.len()),
            };
            let len = u32::try_from(len).expect("a copied object fits its header");
            let handle = scope.alloc(class, payload, len);
            if let Body::Bytes(bytes) = &node.body {
                scope.bytes_mut(handle).copy_from_slice(bytes);
            }
            handles.push(handle);
        }
        let value = |scope: &HandleScope<'h>, item: Item, handles: &[super::Handle<'h>]| match item
        {
            Item::Imm(value) => value,
            Item::Class(id) => scope.class_object(id),
            Item::Node(index) => scope.get(handles[index as usize]),
            Item::Known(value) => value,
        };
        let hash = ClassId::from_index(Builtin::Hash as u32);
        let mut hashes = Vec::new();
        let mut frozen = Vec::new();
        for (node, &handle) in msg.nodes.iter().zip(&handles) {
            match &node.body {
                Body::Slots(slots) | Body::Regexp(_, slots) => {
                    for (index, &item) in slots.iter().enumerate() {
                        let slot = value(scope, item, &handles);
                        scope.set_slot(handle, index, slot);
                    }
                }
                Body::Bytes(_) => {}
            }
            if let Body::Regexp(compiled, _) = &node.body {
                let index = scope.regexps_mut().adopt(Arc::clone(compiled));
                let index = Value::fixnum(index as i64).expect("a pattern index is a fixnum");
                scope.set_slot(handle, crate::regexp::REGEXP_INDEX, index);
            }
            if let Some(names) = &node.shape {
                let mut shape = ShapeId::ROOT;
                for &name in names {
                    shape = match scope.shapes_mut().transition(shape, name) {
                        Some((grown, _)) => grown,
                        None => break,
                    };
                }
                scope.set_shape(handle, shape);
            }
            if let Some(side) = node.side {
                let storage = value(scope, side, &handles);
                scope.set_side_ivars(handle, storage);
            }
            scope.set_copied_flags(handle, node.flags & !FROZEN);
            if node.flags & FROZEN != 0 {
                if node.class.is_some() {
                    frozen.push(scope.get(handle));
                } else {
                    // A buffer more than one String shares stays shared.
                    scope.freeze(handle);
                }
            }
            if node
                .class
                .is_some_and(|id| scope.classes().reaches(id, hash))
            {
                hashes.push(scope.get(handle));
            }
        }
        let root = value(scope, msg.root, &handles);
        let nodes = handles.iter().map(|&handle| scope.get(handle)).collect();
        Imported {
            root,
            hashes,
            frozen,
            nodes,
        }
    }

    /// Give a copied object the flags its original had.
    fn set_copied_flags(&mut self, handle: super::Handle<'h>, flags: u8) {
        let object = self.object(handle);
        // SAFETY: the handle roots a live object of this heap.
        unsafe { (*object.header()).flags |= flags & COPIED_FLAGS };
    }
}
