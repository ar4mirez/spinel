//! The per-heap shape tree: what an object's instance variables are called, and
//! in what order it acquired them.
//!
//! An instance variable is not a slot number the compiler can pick. Two objects
//! of one class hold different sets of them, so the map from name to storage
//! index belongs to the object. Putting a hash table in every object would cost
//! a word and a lookup per read, so objects that were *built the same way*
//! share one description of their layout instead and carry a `u16` id for it —
//! V8's hidden classes, CRuby 3.2's shapes.
//!
//! A shape is a path from the root. Each edge adds one name at one index, so
//! `@a` then `@b` is a different shape from `@b` then `@a`. That divergence is
//! the mechanism rather than a wart: it is what makes an index constant for
//! every object wearing a shape.
//!
//! The tree holds symbols and indices and no [`Value`](crate::value::Value), so
//! it is not a root source in [`Heap::mark`](crate::heap::Heap). Where the
//! values themselves live is `interp.rs`'s business — one slot of the object,
//! pointing at storage that is replaced when it grows, the way an `Array`'s
//! elements already work.

use std::collections::HashMap;

use crate::value::SymbolId;

/// An index into a heap's shape tree, small enough for the two bytes #7
/// reserved in the object header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct ShapeId(pub u16);

impl ShapeId {
    /// The object cannot hold instance variables at all.
    ///
    /// Not the same as "holds none yet", and the difference is load-bearing:
    /// slot 0 of an `Array` is its element storage and slot 0 of a `Proc` is
    /// its iseq. Without a value that means "do not look", `@x = 1` on an array
    /// would quietly overwrite the array.
    pub const NONE: ShapeId = ShapeId(0);

    /// Ivar-capable, holding none. Every object's shape starts here and walks
    /// away from it one name at a time.
    pub const ROOT: ShapeId = ShapeId(1);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug)]
struct Shape {
    /// The shape this one was reached from. `ROOT`'s parent is itself, which
    /// terminates every walk without a branch.
    parent: ShapeId,
    /// The name this edge adds. Meaningless on `ROOT` and on `NONE`.
    name: SymbolId,
    /// Where `name` lands in the object's storage.
    index: u16,
    /// Instance variables on this shape, counting inherited ones. Also the
    /// index the next transition would use.
    count: u16,
    /// Names already transitioned away from here, so two objects built the same
    /// way meet at the same node instead of each growing their own.
    children: HashMap<SymbolId, ShapeId>,
}

/// One heap's shape tree.
#[derive(Debug)]
pub struct Shapes {
    nodes: Vec<Shape>,
}

impl Default for Shapes {
    fn default() -> Shapes {
        Shapes::new()
    }
}

impl Shapes {
    pub fn new() -> Shapes {
        let sentinel = Shape {
            parent: ShapeId::NONE,
            name: SymbolId(0),
            index: 0,
            count: 0,
            children: HashMap::new(),
        };
        let root = Shape {
            parent: ShapeId::ROOT,
            name: SymbolId(0),
            index: 0,
            count: 0,
            children: HashMap::new(),
        };
        Shapes {
            nodes: vec![sentinel, root],
        }
    }

    /// Shapes in the tree, counting both reserved nodes. `GC.stat`-adjacent,
    /// and what the tests assert sharing with.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Instance variables an object wearing `shape` holds.
    pub fn count(&self, shape: ShapeId) -> u16 {
        self.nodes[shape.index()].count
    }

    /// Where `name` lives in the storage of an object wearing `shape`, if it
    /// holds one by that name.
    ///
    /// A walk rather than a per-shape map: the chain is as long as the object
    /// has instance variables, which is a handful.
    //
    // ponytail: linear in the ivar count, on every read. `docs/engine.md`'s
    // call-site cache keyed by shape id is the upgrade, and the benchmark that
    // would justify writing it arrives with the JIT.
    pub fn index_of(&self, shape: ShapeId, name: SymbolId) -> Option<u16> {
        let mut at = shape;
        while at != ShapeId::ROOT && at != ShapeId::NONE {
            let node = &self.nodes[at.index()];
            if node.name == name {
                return Some(node.index);
            }
            at = node.parent;
        }
        None
    }

    /// The shape an object wearing `shape` has after gaining `name`, and the
    /// index to write it at.
    ///
    /// `None` when the tree is full. 65,534 shapes is a heap that has built
    /// objects 65,534 distinct ways; growing the field past `u16` pushes the
    /// header to 24 bytes and shifts every size class, which is a thing to
    /// decide with a workload in hand rather than here.
    pub fn transition(&mut self, shape: ShapeId, name: SymbolId) -> Option<(ShapeId, u16)> {
        debug_assert!(shape != ShapeId::NONE, "a transition off the sentinel");
        if let Some(&child) = self.nodes[shape.index()].children.get(&name) {
            return Some((child, self.nodes[child.index()].index));
        }
        let index = self.nodes[shape.index()].count;
        let id = ShapeId(u16::try_from(self.nodes.len()).ok()?);
        self.nodes.push(Shape {
            parent: shape,
            name,
            index,
            count: index.checked_add(1)?,
            children: HashMap::new(),
        });
        self.nodes[shape.index()].children.insert(name, id);
        Some((id, index))
    }

    /// The names an object wearing `shape` holds, in the order it acquired
    /// them — which is the order `Object#instance_variables` answers in.
    pub fn names(&self, shape: ShapeId) -> Vec<SymbolId> {
        let mut names = vec![SymbolId(0); self.count(shape) as usize];
        let mut at = shape;
        while at != ShapeId::ROOT && at != ShapeId::NONE {
            let node = &self.nodes[at.index()];
            names[node.index as usize] = node.name;
            at = node.parent;
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(n: u32) -> SymbolId {
        SymbolId(n)
    }

    #[test]
    fn the_root_holds_nothing_and_knows_nothing() {
        let shapes = Shapes::new();
        assert_eq!(shapes.count(ShapeId::ROOT), 0);
        assert_eq!(shapes.index_of(ShapeId::ROOT, sym(1)), None);
        assert!(shapes.names(ShapeId::ROOT).is_empty());
    }

    #[test]
    fn the_same_names_in_the_same_order_reach_the_same_shape() {
        let mut shapes = Shapes::new();
        let (a1, i1) = shapes.transition(ShapeId::ROOT, sym(1)).unwrap();
        let (b1, j1) = shapes.transition(a1, sym(2)).unwrap();
        let (a2, i2) = shapes.transition(ShapeId::ROOT, sym(1)).unwrap();
        let (b2, j2) = shapes.transition(a2, sym(2)).unwrap();
        assert_eq!((a1, b1), (a2, b2));
        assert_eq!((i1, j1), (i2, j2));
        assert_eq!((i1, j1), (0, 1));
    }

    #[test]
    fn the_same_names_in_a_different_order_do_not() {
        let mut shapes = Shapes::new();
        let (a, _) = shapes.transition(ShapeId::ROOT, sym(1)).unwrap();
        let (ab, _) = shapes.transition(a, sym(2)).unwrap();
        let (b, _) = shapes.transition(ShapeId::ROOT, sym(2)).unwrap();
        let (ba, _) = shapes.transition(b, sym(1)).unwrap();
        assert_ne!(ab, ba);
        // Both hold both names, at opposite indices. That is the whole reason
        // the two shapes cannot be merged.
        assert_eq!(shapes.index_of(ab, sym(1)), Some(0));
        assert_eq!(shapes.index_of(ab, sym(2)), Some(1));
        assert_eq!(shapes.index_of(ba, sym(2)), Some(0));
        assert_eq!(shapes.index_of(ba, sym(1)), Some(1));
    }

    #[test]
    fn a_name_set_twice_does_not_grow_the_object() {
        let mut shapes = Shapes::new();
        let (a, index) = shapes.transition(ShapeId::ROOT, sym(1)).unwrap();
        assert_eq!(shapes.count(a), 1);
        // The caller asks `index_of` first, and only transitions on a miss.
        assert_eq!(shapes.index_of(a, sym(1)), Some(index));
    }

    #[test]
    fn names_come_back_in_the_order_they_were_added() {
        let mut shapes = Shapes::new();
        let (a, _) = shapes.transition(ShapeId::ROOT, sym(7)).unwrap();
        let (b, _) = shapes.transition(a, sym(3)).unwrap();
        let (c, _) = shapes.transition(b, sym(5)).unwrap();
        assert_eq!(shapes.names(c), vec![sym(7), sym(3), sym(5)]);
    }

    #[test]
    fn a_branch_shares_its_prefix() {
        let mut shapes = Shapes::new();
        let (a, _) = shapes.transition(ShapeId::ROOT, sym(1)).unwrap();
        let (ab, _) = shapes.transition(a, sym(2)).unwrap();
        let (ac, _) = shapes.transition(a, sym(3)).unwrap();
        assert_ne!(ab, ac);
        assert_eq!(shapes.index_of(ab, sym(1)), shapes.index_of(ac, sym(1)));
        assert_eq!(shapes.count(ab), shapes.count(ac));
    }
}
