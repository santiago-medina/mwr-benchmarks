//! Two independent arenas with a stop-the-world tracing coordinator.
//! Cross-heap edges are NOT permanent roots. All mutations go through Runtime.
//! This is a protocol experiment, not an integration with Servo or two concurrent GCs.
use crate::{Collection, Error, Handle, Heap, Model, Root};
use std::collections::{HashMap, HashSet};

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
    records: HashMap<Id, Record>,
    budget: Option<usize>,
    stress: bool,
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
            records: HashMap::new(),
            budget: None,
            stress: false,
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
        self.records
            .values()
            .map(|r| {
                Self::allocation_charge(r.payload.len()).unwrap()
                    + r.links.len() * Self::edge_charge()
            })
            .sum()
    }
    fn fits(&self, extra: usize) -> bool {
        self.charged_bytes()
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
        Ok(())
    }
    pub fn clear_edges(&mut self, id: Id) -> Result<(), Error> {
        self.value(id)?;
        self.records.get_mut(&id).unwrap().links.clear();
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

    pub fn collect(&mut self) -> Collected {
        let before = self.charged_bytes();
        let mut pending: Vec<_> = self
            .dom
            .root_handles()
            .into_iter()
            .map(Id::Dom)
            .chain(self.js.root_handles().into_iter().map(Id::Js))
            .collect();
        let mut live = HashSet::new();
        while let Some(id) = pending.pop() {
            if live.insert(id) {
                let record = self
                    .records
                    .get(&id)
                    .expect("root or bridge to missing record");
                pending.extend(record.links.iter().copied());
            }
        }
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
        self.records.retain(|id, _| live.contains(id));
        Collected {
            dom,
            js,
            reclaimed_charge: before - self.charged_bytes(),
        }
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
