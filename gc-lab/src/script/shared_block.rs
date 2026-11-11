//! Shared Data Blocks (ECMA-262 §6.2.9) that several agents (threads, one engine each) can
//! reach, their WaiterLists (§25.4.1) and the per-agent mailbox that delivers cross-agent host
//! events. Design: AGENTS.md.
//!
//! Every access to a shared block goes through its mutex, so the agents observe a sequentially
//! consistent execution, which the memory model (§29) admits for unordered and seq-cst events.
//! No JS code ever runs while a block or mailbox lock is held.
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::Instant,
};

/// The bytes behind an ArrayBuffer (`Local`) or a SharedArrayBuffer (`Shared`).
pub(super) enum Bytes {
    Local(Vec<u8>),
    Shared(Arc<SharedBlock>),
}
impl Bytes {
    pub fn len(&self) -> usize {
        match self {
            Bytes::Local(v) => v.len(),
            Bytes::Shared(b) => b.lock().bytes.len(),
        }
    }
    /// Read access. Never call back into the engine from `f`.
    pub fn with<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        match self {
            Bytes::Local(v) => f(v),
            Bytes::Shared(b) => f(&b.lock().bytes),
        }
    }
    /// Write access (also length changes). Never call back into the engine from `f`.
    pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut Vec<u8>) -> R) -> R {
        match self {
            Bytes::Local(v) => f(v),
            Bytes::Shared(b) => f(&mut b.lock().bytes),
        }
    }
    /// A copy of `start..end`.
    pub fn get(&self, start: usize, end: usize) -> Vec<u8> {
        self.with(|s| s[start..end].to_vec())
    }
    /// Overwrite `values.len()` bytes at `start`.
    pub fn set(&mut self, start: usize, values: &[u8]) {
        self.with_mut(|s| s[start..start + values.len()].copy_from_slice(values))
    }
    pub fn block(&self) -> Option<&Arc<SharedBlock>> {
        match self {
            Bytes::Shared(b) => Some(b),
            Bytes::Local(_) => None,
        }
    }
}

/// How a waiter in a WaiterList is woken (§25.4.1.1 Waiter Record [[PromiseCapability]]).
pub(super) enum WaiterKind {
    /// A synchronous `Atomics.wait`: the agent sleeps on the block's condition variable.
    Sync,
    /// An `Atomics.waitAsync` promise: NotifyWaiter posts `Event::Notified(id)` to the
    /// waiting agent's mailbox (EnqueueResolveInAgentJob, §25.4.1.7).
    Async(Arc<Mailbox>),
}
pub(super) struct SharedWaiter {
    pub id: u64,
    pub byte_index: usize,
    pub kind: WaiterKind,
}

pub(super) struct BlockState {
    pub bytes: Vec<u8>,
    /// All WaiterLists of this block, in insertion (FIFO) order; a list is the subset with
    /// one byte index.
    pub waiters: Vec<SharedWaiter>,
}

/// A Shared Data Block; the mutex is also the WaiterList critical section (§25.4.1.2).
pub(super) struct SharedBlock {
    state: Mutex<BlockState>,
    pub wake: Condvar,
}
impl SharedBlock {
    pub fn new(bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(BlockState {
                bytes,
                waiters: vec![],
            }),
            wake: Condvar::new(),
        })
    }
    /// EnterCriticalSection. A panic in another agent must not wedge this one.
    pub fn lock(&self) -> MutexGuard<'_, BlockState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Unique waiter ids across the whole cluster.
    pub fn next_waiter_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }
}

/// A broadcast from the main agent (test262 `$262.agent.broadcast`).
pub(super) struct Broadcast {
    pub block: Arc<SharedBlock>,
    pub maximum: Option<usize>,
    pub number: f64,
    /// Agents that have not taken the message yet; the parent waits for zero.
    pub ack: Arc<(Mutex<usize>, Condvar)>,
}

pub(super) enum Event {
    Notified(u64),
    Broadcast(Broadcast),
}

/// One per engine: events other agents post to it.
#[derive(Default)]
pub(super) struct Mailbox {
    events: Mutex<VecDeque<Event>>,
    ready: Condvar,
}
impl Mailbox {
    fn events(&self) -> MutexGuard<'_, VecDeque<Event>> {
        self.events.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn post(&self, event: Event) {
        self.events().push_back(event);
        self.ready.notify_all();
    }
    pub fn take(&self) -> Vec<Event> {
        self.events().drain(..).collect()
    }
    pub fn is_empty(&self) -> bool {
        self.events().is_empty()
    }
    /// Block until an event is posted or `deadline` passes.
    pub fn wait(&self, deadline: Instant) {
        let mut events = self.events();
        while events.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            events = self
                .ready
                .wait_timeout(events, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}
