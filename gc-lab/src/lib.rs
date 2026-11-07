//! Three deliberately simple, single-threaded tracing collectors.
//! Handles are checked tokens, not Rust references. Neither model infers stack roots.
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HEAP: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Stable,
    Compact,
    Indirect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    heap: u64,
    index: usize,
    serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    heap: u64,
    index: usize,
    serial: u64,
}

struct RootEntry {
    serial: u64,
    handle: Handle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidHandle,
    InvalidRoot,
}

#[derive(Debug)]
struct Object {
    serial: u64,
    token_index: usize,
    value: i64,
    edges: Vec<Handle>,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Collection {
    pub scanned_slots: usize,
    pub live_objects: usize,
    pub freed_objects: usize,
    pub moved_objects: usize,
    pub rewritten_edges: usize,
    pub refreshed_roots: usize,
    pub table_updates: usize,
}

pub struct Heap {
    id: u64,
    model: Model,
    next_serial: u64,
    slots: Vec<Option<Object>>,
    free: Vec<usize>,
    roots: Vec<Option<RootEntry>>,
    free_roots: Vec<usize>,
    locations: Vec<Option<usize>>,
    stress: bool,
}

impl Heap {
    // Keep compatibility with Rust versions preceding the try_update rename.
    #[allow(deprecated)]
    pub fn new(model: Model) -> Self {
        let id = NEXT_HEAP
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("heap identity exhausted");
        Self {
            id,
            model,
            next_serial: 1,
            slots: vec![],
            free: vec![],
            roots: vec![],
            free_roots: vec![],
            locations: vec![],
            stress: false,
        }
    }

    fn serial(&mut self) -> u64 {
        let serial = self.next_serial;
        self.next_serial = serial.checked_add(1).expect("object identity exhausted");
        serial
    }

    fn position(&self, h: Handle) -> Result<usize, Error> {
        if h.heap != self.id {
            return Err(Error::InvalidHandle);
        }
        let position = if self.model == Model::Indirect {
            self.locations
                .get(h.index)
                .copied()
                .flatten()
                .ok_or(Error::InvalidHandle)?
        } else {
            h.index
        };
        self.slots
            .get(position)
            .and_then(Option::as_ref)
            .filter(|o| o.serial == h.serial)
            .ok_or(Error::InvalidHandle)?;
        Ok(position)
    }

    fn object(&self, h: Handle) -> Result<&Object, Error> {
        Ok(self.slots[self.position(h)?].as_ref().unwrap())
    }

    pub fn allocate(&mut self, value: i64) -> Handle {
        // Collect BEFORE allocation so the freshly returned object is usable until
        // the next safepoint. Callers must root it before another allocation.
        if self.stress {
            self.collect();
        }
        let serial = self.serial();
        if self.model == Model::Indirect {
            let index = self.free.pop().unwrap_or_else(|| {
                self.locations.push(None);
                self.locations.len() - 1
            });
            self.locations[index] = Some(self.slots.len());
            self.slots.push(Some(Object {
                serial,
                token_index: index,
                value,
                edges: vec![],
            }));
            return Handle {
                heap: self.id,
                index,
                serial,
            };
        }
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            self.slots.push(None);
            self.slots.len() - 1
        };
        self.slots[index] = Some(Object {
            serial,
            token_index: index,
            value,
            edges: vec![],
        });
        Handle {
            heap: self.id,
            index,
            serial,
        }
    }

    pub fn value(&self, h: Handle) -> Result<i64, Error> {
        Ok(self.object(h)?.value)
    }

    pub fn link(&mut self, from: Handle, to: Handle) -> Result<(), Error> {
        self.object(to)?;
        let position = self.position(from)?;
        self.slots[position].as_mut().unwrap().edges.push(to);
        Ok(())
    }

    pub fn edges(&self, h: Handle) -> Result<Vec<Handle>, Error> {
        Ok(self.object(h)?.edges.clone())
    }

    /// Unrooted handles do not keep objects alive, in any model.
    pub fn root(&mut self, h: Handle) -> Result<Root, Error> {
        self.object(h)?;
        let serial = self.serial();
        let entry = Some(RootEntry { serial, handle: h });
        let index = if let Some(index) = self.free_roots.pop() {
            self.roots[index] = entry;
            index
        } else {
            self.roots.push(entry);
            self.roots.len() - 1
        };
        Ok(Root {
            heap: self.id,
            index,
            serial,
        })
    }

    pub fn resolve(&self, r: Root) -> Result<Handle, Error> {
        if r.heap != self.id {
            return Err(Error::InvalidRoot);
        }
        self.roots
            .get(r.index)
            .and_then(Option::as_ref)
            .filter(|entry| entry.serial == r.serial)
            .map(|entry| entry.handle)
            .ok_or(Error::InvalidRoot)
    }

    pub fn unroot(&mut self, r: Root) -> Result<(), Error> {
        self.resolve(r)?;
        self.roots[r.index] = None;
        self.free_roots.push(r.index);
        Ok(())
    }

    /// Explicit callback boundary; stress mode also collects before each allocation.
    /// Roots survive reentry and collection. Raw handles may not survive compaction.
    pub fn call<T>(&mut self, r: Root, callback: impl FnOnce(&mut Self, Root) -> T) -> T {
        callback(self, r)
    }

    pub fn collect(&mut self) -> Collection {
        let mut marked = vec![false; self.slots.len()];
        let mut pending: Vec<_> = self
            .roots
            .iter()
            .flatten()
            .map(|entry| entry.handle)
            .collect();
        while let Some(h) = pending.pop() {
            let obj = self.object(h).expect("internal dangling edge or root");
            let position = self.position(h).unwrap();
            if marked[position] {
                continue;
            }
            marked[position] = true;
            pending.extend(obj.edges.iter().copied());
        }
        let mut stats = Collection {
            scanned_slots: self.slots.len(),
            ..Collection::default()
        };
        if self.model != Model::Indirect {
            self.free.clear();
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if marked[index] {
                stats.live_objects += 1;
            } else {
                if let Some(object) = slot.take() {
                    stats.freed_objects += 1;
                    if self.model == Model::Indirect {
                        self.locations[object.token_index] = None;
                        self.free.push(object.token_index);
                    }
                }
                if self.model == Model::Stable {
                    self.free.push(index);
                }
            }
        }
        if self.model == Model::Stable {
            return stats;
        }

        if self.model == Model::Indirect {
            let mut dense = Vec::with_capacity(stats.live_objects);
            for (old_position, object) in std::mem::take(&mut self.slots).into_iter().enumerate() {
                if let Some(object) = object {
                    let position = dense.len();
                    if position != old_position {
                        stats.moved_objects += 1;
                    }
                    self.locations[object.token_index] = Some(position);
                    stats.table_updates += 1;
                    dense.push(Some(object));
                }
            }
            self.slots = dense;
            return stats;
        }

        // New tokens invalidate every pre-collection raw handle, even if its index
        // happens to stay unchanged. No dangling access can silently alias another object.
        let mut relocation = vec![None; self.slots.len()];
        let mut dense = Vec::with_capacity(stats.live_objects);
        for (old_index, slot) in std::mem::take(&mut self.slots).into_iter().enumerate() {
            if let Some(mut object) = slot {
                let index = dense.len();
                if index != old_index {
                    stats.moved_objects += 1;
                }
                object.serial = self.serial();
                object.token_index = index;
                relocation[old_index] = Some(Handle {
                    heap: self.id,
                    index,
                    serial: object.serial,
                });
                dense.push(Some(object));
            }
        }
        for object in dense.iter_mut().flatten() {
            for edge in &mut object.edges {
                *edge = relocation[edge.index].expect("reachable edge not marked");
                stats.rewritten_edges += 1;
            }
        }
        for entry in self.roots.iter_mut().flatten() {
            entry.handle = relocation[entry.handle.index].expect("root not marked");
            stats.refreshed_roots += 1;
        }
        self.slots = dense;
        self.free.clear();
        stats
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn set_stress(&mut self, enabled: bool) {
        self.stress = enabled;
    }
    pub fn live_roots(&self) -> usize {
        self.roots.iter().flatten().count()
    }
    pub fn root_slot_count(&self) -> usize {
        self.roots.len()
    }
    pub fn table_capacity_bytes(&self) -> usize {
        self.locations.capacity() * std::mem::size_of::<Option<usize>>()
    }
    pub fn edge(&self, h: Handle, index: usize) -> Result<Handle, Error> {
        self.object(h)?
            .edges
            .get(index)
            .copied()
            .ok_or(Error::InvalidHandle)
    }
    pub fn scope<T>(&mut self, f: impl FnOnce(&mut RootScope<'_>) -> T) -> T {
        f(&mut RootScope {
            heap: self,
            owned: vec![],
        })
    }

    /// Container payload capacity only, NOT RSS or allocator overhead.
    /// Free lists and adjacency capacities are included; temporary GC buffers excluded.
    pub fn retained_capacity_bytes(&self) -> usize {
        self.slots.capacity() * std::mem::size_of::<Option<Object>>()
            + self.free.capacity() * std::mem::size_of::<usize>()
            + self.roots.capacity() * std::mem::size_of::<Option<RootEntry>>()
            + self.free_roots.capacity() * std::mem::size_of::<usize>()
            + self.table_capacity_bytes()
            + self
                .slots
                .iter()
                .flatten()
                .map(|o| o.edges.capacity() * std::mem::size_of::<Handle>())
                .sum::<usize>()
    }
}

/// Only roots created through RootScope::root are owned by this scope.
/// Nested scopes do not disturb outer roots. Escaped root tokens become invalid.
pub struct RootScope<'a> {
    heap: &'a mut Heap,
    owned: Vec<Root>,
}

impl RootScope<'_> {
    pub fn root(&mut self, h: Handle) -> Result<Root, Error> {
        let root = self.heap.root(h)?;
        self.owned.push(root);
        Ok(root)
    }
}

impl std::ops::Deref for RootScope<'_> {
    type Target = Heap;
    fn deref(&self) -> &Heap {
        self.heap
    }
}

impl std::ops::DerefMut for RootScope<'_> {
    fn deref_mut(&mut self) -> &mut Heap {
        self.heap
    }
}

impl Drop for RootScope<'_> {
    fn drop(&mut self) {
        for root in self.owned.drain(..) {
            // A caller may already have explicitly released this root.
            let _ = self.heap.unroot(root);
        }
    }
}

pub mod operations;
