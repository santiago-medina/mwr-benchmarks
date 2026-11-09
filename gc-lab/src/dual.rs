//! Two independent arenas with a stop-the-world tracing coordinator.
//! Cross-heap edges are NOT permanent roots. All mutations go through Runtime.
//! This is a protocol experiment, not an integration with Servo or two concurrent GCs.
use crate::{Collection, Error, Handle, Heap, Model, Root};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// Deterministic multiply-rotate hasher for `Id` keys (GC_SCHEDULING.md). `Id`s are
/// internal allocation counters, never attacker-chosen, so SipHash's HashDoS
/// resistance buys nothing here and it dominated GC profiles.
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);
impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }
    fn write_u64(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_usize(&mut self, word: usize) {
        self.write_u64(word as u64);
    }
    fn write_isize(&mut self, word: isize) {
        self.write_u64(word as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
pub type BuildIdHasher = BuildHasherDefault<IdHasher>;
pub type IdMap<V> = HashMap<Id, V, BuildIdHasher>;
pub type IdSet = HashSet<Id, BuildIdHasher>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Dom,
    Js,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Id {
    Dom(Handle),
    Js(Handle),
}

impl Id {
    fn domain(self) -> Domain {
        match self {
            Self::Dom(_) => Domain::Dom,
            Self::Js(_) => Domain::Js,
        }
    }
    fn handle(self) -> Handle {
        match self {
            Self::Dom(h) | Self::Js(h) => h,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalRoot {
    id: Id,
    token: Root,
}

struct Record {
    payload: Vec<u8>,
    links: Vec<Id>,
}

#[derive(Debug)]
pub struct Collected {
    pub dom: Collection,
    pub js: Collection,
    pub reclaimed_charge: usize,
}

pub struct Runtime {
    dom: Heap,
    js: Heap,
    records: IdMap<Record>,
    budget: Option<usize>,
    stress: bool,
    foreign_roots: IdSet,
    /// Running total of `charged_bytes()`; kept in O(1) (see GC_SCHEDULING.md).
    charged: usize,
    collections: u64,
}

impl Runtime {
    /// Stable IDs are a prerequisite. Direct-token compaction is deliberately
    /// excluded rather than silently leaving cross-heap references stale.
    pub fn new(dom: Model, js: Model) -> Result<Self, Error> {
        if dom == Model::Compact || js == Model::Compact {
            return Err(Error::InvalidHandle);
        }
        Ok(Self {
            dom: Heap::new(dom),
            js: Heap::new(js),
            records: IdMap::default(),
            budget: None,
            stress: false,
            foreign_roots: IdSet::default(),
            charged: 0,
            collections: 0,
        })
    }

    fn heap(&self, domain: Domain) -> &Heap {
        match domain {
            Domain::Dom => &self.dom,
            Domain::Js => &self.js,
        }
    }
    fn heap_mut(&mut self, domain: Domain) -> &mut Heap {
        match domain {
            Domain::Dom => &mut self.dom,
            Domain::Js => &mut self.js,
        }
    }

    pub fn set_budget(&mut self, bytes: Option<usize>) {
        self.budget = bytes;
    }
    pub fn set_stress(&mut self, enabled: bool) {
        self.stress = enabled;
    }
    pub fn allocation_charge(payload: usize) -> Option<usize> {
        std::mem::size_of::<Record>()
            .checked_add(std::mem::size_of::<Id>())?
            .checked_add(Heap::object_charge())?
            .checked_add(payload)
    }
    pub fn edge_charge() -> usize {
        std::mem::size_of::<Id>()
    }
    pub fn charged_bytes(&self) -> usize {
        self.charged
    }
    /// Number of joint collections so far, including budget-triggered ones.
    pub fn collections(&self) -> u64 {
        self.collections
    }
    fn recount_charge(&self) -> usize {
        self.records
            .values()
            .map(|r| {
                Self::allocation_charge(r.payload.len()).unwrap()
                    + r.links.len() * Self::edge_charge()
            })
            .sum()
    }
    fn fits(&self, extra: usize) -> bool {
        self.charged
            .checked_add(extra)
            .is_some_and(|total| self.budget.is_none_or(|limit| total <= limit))
    }

    /// Allocation is a safepoint even if it returns an error. Budget tracks logical
    /// object/record/key/payload/edge bytes, not retained capacities, roots or RSS.
    pub fn allocate(&mut self, domain: Domain, value: i64, payload: usize) -> Result<Id, Error> {
        if self.stress {
            self.collect();
        }
        let charge = Self::allocation_charge(payload).ok_or(Error::ByteLimit)?;
        if !self.fits(charge) {
            if !self.stress {
                self.collect();
            }
            if !self.fits(charge) {
                return Err(Error::ByteLimit);
            }
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(payload)
            .map_err(|_| Error::AllocationFailed)?;
        bytes.resize(payload, 0);
        self.records
            .try_reserve(1)
            .map_err(|_| Error::AllocationFailed)?;
        let h = self.heap_mut(domain).try_allocate(value)?;
        let id = match domain {
            Domain::Dom => Id::Dom(h),
            Domain::Js => Id::Js(h),
        };
        self.records.insert(
            id,
            Record {
                payload: bytes,
                links: vec![],
            },
        );
        self.charged += charge;
        Ok(id)
    }

    pub fn value(&self, id: Id) -> Result<i64, Error> {
        if !self.records.contains_key(&id) {
            return Err(Error::InvalidHandle);
        }
        self.heap(id.domain()).value(id.handle())
    }
    pub fn edges(&self, id: Id) -> Result<Vec<Id>, Error> {
        self.value(id)?;
        Ok(self.records.get(&id).unwrap().links.clone())
    }
    /// Linking is not a safepoint: it either adds one charged edge or fails without
    /// mutation. This lets a freshly returned unrooted object be attached safely.
    pub fn link(&mut self, from: Id, to: Id) -> Result<(), Error> {
        self.value(from)?;
        self.value(to)?;
        if !self.fits(Self::edge_charge()) {
            return Err(Error::ByteLimit);
        }
        let links = &mut self.records.get_mut(&from).unwrap().links;
        links.try_reserve(1).map_err(|_| Error::AllocationFailed)?;
        links.push(to);
        self.charged += Self::edge_charge();
        Ok(())
    }
    pub fn clear_edges(&mut self, id: Id) -> Result<(), Error> {
        self.value(id)?;
        let links = &mut self.records.get_mut(&id).unwrap().links;
        self.charged -= links.len() * Self::edge_charge();
        links.clear();
        Ok(())
    }
    /// Remove one edge, without allocating or collecting.
    pub fn unlink(&mut self, from: Id, to: Id) -> Result<(), Error> {
        self.value(from)?;
        self.value(to)?;
        let links = &mut self.records.get_mut(&from).unwrap().links;
        if let Some(index) = links.iter().position(|id| *id == to) {
            links.remove(index);
            self.charged -= Self::edge_charge();
        }
        Ok(())
    }
    pub fn root(&mut self, id: Id) -> Result<ExternalRoot, Error> {
        self.value(id)?;
        let token = self.heap_mut(id.domain()).root(id.handle())?;
        Ok(ExternalRoot { id, token })
    }
    pub fn resolve(&self, root: ExternalRoot) -> Result<Id, Error> {
        self.heap(root.id.domain()).resolve(root.token)?;
        self.value(root.id)?;
        Ok(root.id)
    }
    pub fn unroot(&mut self, root: ExternalRoot) -> Result<(), Error> {
        self.resolve(root)?;
        self.heap_mut(root.id.domain()).unroot(root.token)
    }
    pub fn live_roots(&self) -> usize {
        self.dom.live_roots() + self.js.live_roots()
    }
    pub fn live_objects(&self) -> usize {
        self.records.len()
    }

    /// Native listeners pin the VM between coordinated collections. These are
    /// NOT seed roots when tracing the combined native/VM graph.
    pub fn pin_foreign(&mut self, id: Id, enabled: bool) -> Result<(), Error> {
        self.value(id)?;
        if enabled {
            self.foreign_roots.insert(id);
        } else {
            self.foreign_roots.remove(&id);
        }
        Ok(())
    }
    pub fn root_ids(&self) -> Vec<Id> {
        self.dom
            .root_handles()
            .into_iter()
            .map(Id::Dom)
            .chain(self.js.root_handles().into_iter().map(Id::Js))
            .collect()
    }
    pub fn reachable(&self, seeds: &[Id]) -> Result<IdSet, Error> {
        let mut pending = seeds.to_vec();
        let mut live = IdSet::default();
        while let Some(id) = pending.pop() {
            self.value(id)?;
            if live.insert(id) {
                pending.extend(self.records[&id].links.iter().copied());
            }
        }
        Ok(live)
    }
    pub fn collect(&mut self) -> Collected {
        let foreign: Vec<_> = self.foreign_roots.iter().copied().collect();
        self.collect_joint(&foreign).expect("valid foreign roots")
    }
    /// The caller has completed native marking, including callbacks back into
    /// this arena. Ignore conservative pins; only real VM roots and native marks
    /// seed this sweep. Validate every seed before changing either heap.
    pub fn collect_joint(&mut self, native: &[Id]) -> Result<Collected, Error> {
        let before = self.charged_bytes();
        let mut seeds = self.root_ids();
        seeds.extend_from_slice(native);
        let live = self.reachable(&seeds)?;
        let dom: Vec<_> = live
            .iter()
            .filter_map(|id| match id {
                Id::Dom(h) => Some(*h),
                _ => None,
            })
            .collect();
        let js: Vec<_> = live
            .iter()
            .filter_map(|id| match id {
                Id::Js(h) => Some(*h),
                _ => None,
            })
            .collect();
        // Mark the complete union before sweeping EITHER arena. Private heaps may
        // never collect independently while bridge edges exist.
        let dom = self.dom.collect_with_roots(&dom);
        let js = self.js.collect_with_roots(&js);
        let mut reclaimed = 0;
        self.records.retain(|id, r| {
            let keep = live.contains(id);
            if !keep {
                reclaimed += Self::allocation_charge(r.payload.len()).unwrap()
                    + r.links.len() * Self::edge_charge();
            }
            keep
        });
        self.charged = before - reclaimed;
        debug_assert_eq!(self.charged, self.recount_charge());
        self.foreign_roots.retain(|id| live.contains(id));
        self.collections += 1;
        Ok(Collected {
            dom,
            js,
            reclaimed_charge: reclaimed,
        })
    }

    pub fn scope<T>(&mut self, f: impl FnOnce(&mut Scope<'_>) -> T) -> T {
        f(&mut Scope {
            runtime: self,
            owned: vec![],
        })
    }
}

pub struct Scope<'a> {
    runtime: &'a mut Runtime,
    owned: Vec<ExternalRoot>,
}
impl Scope<'_> {
    pub fn root(&mut self, id: Id) -> Result<ExternalRoot, Error> {
        let root = self.runtime.root(id)?;
        self.owned.push(root);
        Ok(root)
    }
}
impl std::ops::Deref for Scope<'_> {
    type Target = Runtime;
    fn deref(&self) -> &Runtime {
        self.runtime
    }
}
impl std::ops::DerefMut for Scope<'_> {
    fn deref_mut(&mut self) -> &mut Runtime {
        self.runtime
    }
}
impl Drop for Scope<'_> {
    fn drop(&mut self) {
        for root in self.owned.drain(..) {
            let _ = self.runtime.unroot(root);
        }
    }
}

/// A node and a listener in distinct arenas, with a real cross-arena cycle.
pub fn create_cycle(r: &mut Runtime) -> Result<Id, Error> {
    r.scope(|s| {
        let node = s.allocate(Domain::Dom, -100, 16)?;
        let root = s.root(node)?;
        let callback = s.allocate(Domain::Js, -200, 32)?;
        let node = s.resolve(root)?;
        s.link(node, callback)?;
        s.link(callback, node)?;
        Ok(node)
    })
}

/// Synchronous reentry and object results retaining a foreign node.
pub fn dispatch(r: &mut Runtime, node: ExternalRoot, depth: u8) -> Result<Id, Error> {
    if depth > 32 {
        return Err(Error::ReentryLimit);
    }
    r.scope(|s| {
        let node_id = s.resolve(node)?;
        if node_id.domain() != Domain::Dom || s.value(node_id)? != -100 {
            return Err(Error::InvalidHandle);
        }
        let callback = s
            .edges(node_id)?
            .first()
            .copied()
            .ok_or(Error::InvalidHandle)?;
        if callback.domain() != Domain::Js || s.value(callback)? != -200 {
            return Err(Error::InvalidHandle);
        }
        let _active_callback = s.root(callback)?;
        let captured = s
            .edges(callback)?
            .first()
            .copied()
            .ok_or(Error::InvalidHandle)?;
        if captured.domain() != Domain::Dom || s.value(captured)? != -100 {
            return Err(Error::InvalidHandle);
        }
        let capture_root = s.root(captured)?;
        s.collect();
        let nested = if depth > 0 {
            let nested = dispatch(s, capture_root, depth - 1)?;
            Some(s.root(nested)?)
        } else {
            None
        };
        let result = s.allocate(Domain::Js, i64::from(depth), 64)?;
        let captured = s.resolve(capture_root)?;
        s.link(result, captured)?;
        if let Some(nested) = nested {
            let nested = s.resolve(nested)?;
            s.link(result, nested)?;
        }
        Ok(result)
    })
}
