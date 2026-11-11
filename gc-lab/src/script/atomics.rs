//! Atomics (ECMA-262 §25.4), the waiter lists behind Atomics.wait / waitAsync / notify, and
//! the test262 host object `$262` (harness only). Design: SHARED_MEMORY.md.
//!
//! Shared bytes live in a `SharedBlock` whose mutex is also the WaiterList critical section,
//! so agents on other threads (one engine each, AGENTS.md) can wait and notify. Async waiters
//! are settled through host jobs that the embedder drains with `run_host_jobs` (js_check
//! does; the Servo bridge must too, see SHARED_MEMORY.md).
use super::{
    agents::AgentLink,
    buffer_elements::Element,
    buffers::Store,
    objects::{Descriptor, Key},
    shared_block::{Event, Mailbox, SharedBlock, SharedWaiter, WaiterKind},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::{ExternalRoot, Id};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, ScriptError>;

const ATOMICS_METHODS: &[(&str, usize)] = &[
    ("add", 3),
    ("and", 3),
    ("compareExchange", 4),
    ("exchange", 3),
    ("isLockFree", 1),
    ("load", 2),
    ("notify", 3),
    ("or", 3),
    ("pause", 0),
    ("store", 3),
    ("sub", 3),
    ("wait", 4),
    ("waitAsync", 4),
    ("xor", 3),
];

/// This agent's side of an async waiter (§25.4.3.14 Waiter Record): the promise's resolve
/// function. The record itself sits in the block's WaiterList.
struct Waiter {
    id: u64,
    block: Arc<SharedBlock>,
    resolve: ExternalRoot,
}

enum HostJob {
    /// `setTimeout(callback)` from the test harness host.
    Callback(ExternalRoot),
    /// EnqueueAtomicsWaitAsyncTimeoutJob (§25.4.3.15).
    WaitTimeout(u64),
}

struct Timer {
    deadline: Instant,
    seq: u64,
    job: HostJob,
}

pub(super) struct AtomicsState {
    /// AgentCanSuspend() (§9.7 [[CanBlock]]). False by default: a window's main thread.
    pub can_block: bool,
    pub epoch: Instant,
    waiters: Vec<Waiter>,
    timers: Vec<Timer>,
    next: u64,
    /// Events other agents post to this one (notified async waiters, broadcasts).
    pub mailbox: Arc<Mailbox>,
    /// The test262 agent cluster this engine belongs to, if any (AGENTS.md).
    pub agent: AgentLink,
}
impl Default for AtomicsState {
    fn default() -> Self {
        Self {
            can_block: false,
            epoch: Instant::now(),
            waiters: vec![],
            timers: vec![],
            next: 0,
            mailbox: Arc::default(),
            agent: AgentLink::None,
        }
    }
}

/// The typed array facts an atomic access needs.
struct Access {
    store: Store,
    kind: Element,
    byte_index: usize,
    typed: Id,
}

fn raw(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .enumerate()
        .fold(0, |acc, (i, b)| acc | (*b as u64) << (i * 8))
}

impl Engine<'_> {
    pub fn set_can_block(&mut self, can_block: bool) {
        self.atomics.can_block = can_block;
    }
    /// The earliest pending host job (timer, async-waiter settlement or an event another
    /// agent posted), if any.
    pub fn next_host_job(&self) -> Option<Instant> {
        if !self.atomics.mailbox.is_empty() {
            return Some(Instant::now());
        }
        self.atomics.timers.iter().map(|t| t.deadline).min()
    }
    /// Block until another agent posts an event or `deadline` passes; embedders call it
    /// instead of sleeping until `next_host_job()` (AGENTS.md).
    pub fn wait_for_host_event(&self, deadline: Instant) {
        self.atomics.mailbox.wait(deadline);
    }
    /// Async waiters (`Atomics.waitAsync`) that another agent may still notify.
    pub fn has_pending_waiters(&self) -> bool {
        !self.atomics.waiters.is_empty() && self.has_peer_agents()
    }
    /// Run the events other agents posted, then every host job whose deadline has passed,
    /// in deadline then FIFO order.
    pub fn run_host_jobs(&mut self) -> Result<()> {
        let mut first = Ok(());
        for event in self.atomics.mailbox.take() {
            self.steps_for_external_event();
            let result = match event {
                // EnqueueResolveInAgentJob (§25.4.1.7) from NotifyWaiter.
                Event::Notified(id) => match self.atomics.waiters.iter().position(|w| w.id == id) {
                    Some(at) => {
                        let w = self.atomics.waiters.remove(at);
                        self.run_rooted(w.resolve, Some("ok"))
                    }
                    None => Ok(()),
                },
                Event::Broadcast(message) => self.receive_broadcast(message),
            };
            if first.is_ok() {
                first = result;
            }
        }
        let now = Instant::now();
        let mut due: Vec<Timer> = vec![];
        let mut rest = vec![];
        for t in std::mem::take(&mut self.atomics.timers) {
            if t.deadline <= now {
                due.push(t);
            } else {
                rest.push(t);
            }
        }
        self.atomics.timers = rest;
        due.sort_by_key(|t| (t.deadline, t.seq));
        for timer in due {
            self.steps_for_external_event();
            let result = match timer.job {
                HostJob::Callback(root) => self.run_rooted(root, None),
                HostJob::WaitTimeout(id) => {
                    match self.atomics.waiters.iter().position(|w| w.id == id) {
                        Some(at) => {
                            // Under the critical section: still listed → timed out; else a
                            // notification is already in this agent's mailbox.
                            let listed = {
                                let block = self.atomics.waiters[at].block.clone();
                                let mut state = block.lock();
                                let before = state.waiters.len();
                                state.waiters.retain(|w| w.id != id);
                                state.waiters.len() != before
                            };
                            if listed {
                                let w = self.atomics.waiters.remove(at);
                                self.run_rooted(w.resolve, Some("timed-out"))
                            } else {
                                Ok(())
                            }
                        }
                        None => Ok(()),
                    }
                }
            };
            if first.is_ok() {
                first = result;
            }
        }
        first
    }
    fn run_rooted(&mut self, root: ExternalRoot, argument: Option<&'static str>) -> Result<()> {
        let f = self.runtime.resolve(root)?;
        self.frame(|e| {
            e.protect(&Value::Object(f))?;
            e.runtime.unroot(root)?;
            let args = argument.map(|s| vec![Value::String(s.into())]);
            e.invoke_this(Value::Object(f), Value::Undefined, args.unwrap_or_default())
                .map(|_| ())
        })
    }
    fn add_host_job(&mut self, delay: Duration, job: HostJob) {
        let seq = self.atomics.next;
        self.atomics.next += 1;
        self.atomics.timers.push(Timer {
            deadline: Instant::now() + delay,
            seq,
            job,
        });
    }
    pub(super) fn cancel_host_jobs(&mut self) {
        for t in std::mem::take(&mut self.atomics.timers) {
            if let HostJob::Callback(root) = t.job {
                let _ = self.runtime.unroot(root);
            }
        }
        for w in std::mem::take(&mut self.atomics.waiters) {
            w.block.lock().waiters.retain(|x| x.id != w.id);
            let _ = self.runtime.unroot(w.resolve);
        }
        self.release_agent_roots();
    }

    pub(super) fn bootstrap_atomics(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let object = self.intrinsics.as_ref().unwrap().object;
        let function = self.intrinsics.as_ref().unwrap().function;
        let atomics = self.new_object(Some(object))?;
        self.runtime.link(env, atomics)?;
        objects.insert("Atomics".into(), atomics);
        // §25.4.17 Atomics [ %Symbol.toStringTag% ].
        self.define(
            atomics,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Atomics".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        for (method, length) in ATOMICS_METHODS {
            self.host_method(env, function, atomics, "Atomics", method, *length)?;
        }
        if !self.test_harness {
            return Ok(());
        }
        // test262 INTERPRETING.md host-defined `$262`, plus `setTimeout` so that
        // atomicsHelper.js does not fall back to a busy promise loop.
        let host = self.new_object(Some(object))?;
        self.runtime.link(env, host)?;
        objects.insert("$262".into(), host);
        let abstract_source = self.intrinsics.as_ref().unwrap().functions["%AbstractModuleSource%"];
        self.put_data(host, "AbstractModuleSource", Value::Object(abstract_source))?;
        for (method, length) in [
            ("detachArrayBuffer", 1),
            ("gc", 0),
            ("createRealm", 0),
            ("evalScript", 1),
        ] {
            self.host_method(env, function, host, "$262", method, length)?;
        }
        let agent = self.new_object(Some(object))?;
        self.runtime.link(env, agent)?;
        self.put_data(host, "agent", Value::Object(agent))?;
        // INTERPRETING.md: the main agent's API, or the one a started agent sees.
        let methods: &[(&str, usize)] = if self.is_child_agent() {
            &[
                ("receiveBroadcast", 1),
                ("report", 1),
                ("leaving", 0),
                ("sleep", 1),
                ("monotonicNow", 0),
            ]
        } else {
            &[
                ("start", 1),
                ("broadcast", 2),
                ("getReport", 0),
                ("sleep", 1),
                ("monotonicNow", 0),
            ]
        };
        for &(method, length) in methods {
            self.host_method(env, function, agent, "$262.agent", method, length)?;
        }
        let timeout = self.intrinsic_function(env, "setTimeout", Some(function))?;
        objects.insert("setTimeout".into(), timeout);
        Ok(())
    }
    /// `$262.global` once the global object exists.
    pub(super) fn bootstrap_host_global(&mut self, host: Id, global: Id) -> Result<()> {
        self.put_data(host, "global", Value::Object(global))
    }
    fn host_method(
        &mut self,
        env: Id,
        function: Id,
        owner: Id,
        prefix: &str,
        method: &str,
        length: usize,
    ) -> Result<()> {
        let full = format!("{prefix}.{method}");
        let f = self.intrinsic_function(env, &full, Some(function))?;
        self.define(
            f,
            "length",
            Descriptor {
                value: Some(Value::number(length as f64)),
                ..Default::default()
            },
        )?;
        self.hidden(owner, method, Value::Object(f))
    }

    pub(super) fn atomics_builtin(
        &mut self,
        name: &str,
        _receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let arg = |i| args.get(i).cloned().unwrap_or(Value::Undefined);
        for a in &args {
            self.protect(a)?;
        }
        match name {
            "setTimeout" => {
                let callback = arg(0);
                if !self.callable(&callback) {
                    return self.throw_error("TypeError", "setTimeout callback is not callable");
                }
                let delay = self.to_number(arg(1))?;
                let delay = if delay.is_finite() && delay > 0.0 {
                    delay
                } else {
                    0.0
                };
                let root = self.runtime.root(callback.id().unwrap())?;
                self.add_host_job(
                    Duration::from_secs_f64(delay / 1000.0),
                    HostJob::Callback(root),
                );
                Ok(Value::Undefined)
            }
            "$262.detachArrayBuffer" => {
                // DetachArrayBuffer (§25.1.3.5) with key undefined.
                let buffer = self.buffer_slot(arg(0))?;
                if buffer.borrow().shared {
                    return self.throw_error("TypeError", "cannot detach a SharedArrayBuffer");
                }
                if buffer.borrow().immutable {
                    return self.throw_error("TypeError", "cannot detach an immutable ArrayBuffer");
                }
                Self::detach_buffer(&mut buffer.borrow_mut());
                Ok(Value::Null)
            }
            "$262.gc" => {
                self.collect();
                Ok(Value::Undefined)
            }
            "$262.createRealm" => self.create_realm(),
            "$262.evalScript" => self.eval_script(arg(0)),
            "$262.agent.start"
            | "$262.agent.broadcast"
            | "$262.agent.getReport"
            | "$262.agent.receiveBroadcast"
            | "$262.agent.report"
            | "$262.agent.leaving" => self.agent_builtin(name, arg(0), arg(1)),
            "$262.agent.sleep" => {
                let ms = self.to_number(arg(0))?;
                if ms.is_finite() && ms > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(ms.min(10_000.0) / 1000.0));
                }
                Ok(Value::Undefined)
            }
            "$262.agent.monotonicNow" => Ok(Value::number(
                self.atomics.epoch.elapsed().as_secs_f64() * 1000.0,
            )),
            "Atomics.isLockFree" => {
                // §25.4.8: 4 must be lock-free; 1, 2 and 8 are lock-free here too.
                let n = self.integer(arg(0))?;
                Ok(Value::Bool(
                    matches!(n as i64, 1 | 2 | 4 | 8) && n.trunc() == n,
                ))
            }
            "Atomics.pause" => {
                // §25.4.10 Atomics.pause ( [ N ] ).
                let n = arg(0);
                if n != Value::Undefined {
                    let integral = matches!(n, Value::Number(bits)
                        if { let f = f64::from_bits(bits); f.is_finite() && f.trunc() == f });
                    if !integral {
                        return self
                            .throw_error("TypeError", "Atomics.pause hint is not an integer");
                    }
                }
                Ok(Value::Undefined)
            }
            "Atomics.load" => {
                // §25.4.9 Atomics.load ( typedArray, index ).
                let access = self.atomic_access(arg(0), arg(1), false, false)?;
                self.revalidate_atomic_access(&access)?;
                let size = access.kind.size();
                let at = access.byte_index;
                let bytes = access.store.borrow().bytes.get(at, at + size);
                Ok(access.kind.read(&bytes, true))
            }
            "Atomics.store" => {
                // §25.4.12 Atomics.store ( typedArray, index, value ).
                let access = self.atomic_access(arg(0), arg(1), false, true)?;
                let v = self.atomic_operand(access.kind, arg(2))?;
                self.revalidate_atomic_access(&access)?;
                let bytes = access.kind.bytes(&v, true);
                access
                    .store
                    .borrow_mut()
                    .bytes
                    .set(access.byte_index, &bytes);
                Ok(v)
            }
            "Atomics.compareExchange" => {
                // §25.4.5 Atomics.compareExchange ( typedArray, index, expectedValue,
                // replacementValue ).
                let access = self.atomic_access(arg(0), arg(1), false, true)?;
                let expected = self.atomic_operand(access.kind, arg(2))?;
                let replacement = self.atomic_operand(access.kind, arg(3))?;
                self.revalidate_atomic_access(&access)?;
                let expected = access.kind.bytes(&expected, true);
                let replacement = access.kind.bytes(&replacement, true);
                let range = access.byte_index..access.byte_index + access.kind.size();
                // One critical section for the read and the conditional write.
                let old = access.store.borrow_mut().bytes.with_mut(|b| {
                    let old = b[range.clone()].to_vec();
                    if old == expected {
                        b[range].copy_from_slice(&replacement);
                    }
                    old
                });
                Ok(access.kind.read(&old, true))
            }
            "Atomics.add" | "Atomics.and" | "Atomics.exchange" | "Atomics.or" | "Atomics.sub"
            | "Atomics.xor" => {
                // AtomicReadModifyWrite (§25.4.3.17).
                let access = self.atomic_access(arg(0), arg(1), false, true)?;
                let v = self.atomic_operand(access.kind, arg(2))?;
                self.revalidate_atomic_access(&access)?;
                let operand = raw(&access.kind.bytes(&v, true));
                let size = access.kind.size();
                let range = access.byte_index..access.byte_index + size;
                // One critical section for the read and the write.
                let old_bytes = access.store.borrow_mut().bytes.with_mut(|b| {
                    let old_bytes = b[range.clone()].to_vec();
                    let old = raw(&old_bytes);
                    let new = match name {
                        "Atomics.add" => old.wrapping_add(operand),
                        "Atomics.sub" => old.wrapping_sub(operand),
                        "Atomics.and" => old & operand,
                        "Atomics.or" => old | operand,
                        "Atomics.xor" => old ^ operand,
                        _ => operand,
                    };
                    for (i, byte) in b[range].iter_mut().enumerate() {
                        *byte = (new >> (i * 8)) as u8;
                    }
                    old_bytes
                });
                Ok(access.kind.read(&old_bytes, true))
            }
            "Atomics.wait" | "Atomics.waitAsync" => {
                self.do_wait(name == "Atomics.waitAsync", arg(0), arg(1), arg(2), arg(3))
            }
            "Atomics.notify" => self.atomics_notify(arg(0), arg(1), arg(2)),
            _ => Err(ScriptError::Unsupported("Atomics operation")),
        }
    }

    /// ValidateIntegerTypedArray (§25.4.3.1) then ValidateAtomicAccess (§25.4.3.2).
    fn atomic_access(
        &mut self,
        typed: Value,
        index: Value,
        waitable: bool,
        write: bool,
    ) -> Result<Access> {
        let (id, record, length) = self.typed_record_mode(typed, write)?;
        let allowed = if waitable {
            matches!(record.kind, Element::I32 | Element::I64)
        } else {
            !matches!(
                record.kind,
                Element::U8Clamped | Element::F16 | Element::F32 | Element::F64
            )
        };
        if !allowed {
            return self.throw_error(
                "TypeError",
                if waitable {
                    "Atomics wait/notify need an Int32Array or BigInt64Array"
                } else {
                    "Atomics operations need an integer typed array"
                },
            );
        }
        let store = self.data(record.view.buffer)?.buffer.clone().unwrap();
        Ok(Access {
            byte_index: self.atomic_index(index, length, &record)?,
            store,
            kind: record.kind,
            typed: id,
        })
    }
    fn atomic_index(
        &mut self,
        index: Value,
        length: usize,
        record: &super::typed_arrays::Typed,
    ) -> Result<usize> {
        let access = self.to_index(index)?;
        if access >= length as u64 {
            return self.throw_error("RangeError", "atomic access index out of range");
        }
        Ok(access as usize * record.kind.size() + record.view.offset)
    }
    /// RevalidateAtomicAccess (§25.4.3.3): user code may have shrunk or detached the buffer.
    fn revalidate_atomic_access(&mut self, access: &Access) -> Result<()> {
        let record = self.data(access.typed)?.typed.clone().unwrap();
        let Some(length) = self.typed_length(&record)? else {
            return self.throw_error("TypeError", "typed view is detached or out of bounds");
        };
        if access.byte_index >= record.view.offset + length * record.kind.size() {
            return self.throw_error("RangeError", "atomic access index out of range");
        }
        Ok(())
    }
    /// ToBigInt for BigInt element types, else 𝔽(? ToIntegerOrInfinity(v)).
    fn atomic_operand(&mut self, kind: Element, value: Value) -> Result<Value> {
        if kind.big() {
            let n = self.to_bigint(value, false)?;
            self.big_value(n)
        } else {
            let n = self.integer(value)?;
            Ok(Value::number(if n == 0.0 { 0.0 } else { n }))
        }
    }

    /// DoWait ( mode, typedArray, index, value, timeout ) (§25.4.3.14).
    fn do_wait(
        &mut self,
        asynchronous: bool,
        typed: Value,
        index: Value,
        value: Value,
        timeout: Value,
    ) -> Result<Value> {
        // Steps 1-3: ValidateIntegerTypedArray(typedArray, true); the buffer must be shared.
        let (_, record, length) = self.typed_record(typed.clone())?;
        if !matches!(record.kind, Element::I32 | Element::I64) {
            return self.throw_error(
                "TypeError",
                "Atomics wait needs an Int32Array or BigInt64Array",
            );
        }
        let store = self.data(record.view.buffer)?.buffer.clone().unwrap();
        if !store.borrow().shared {
            return self.throw_error("TypeError", "Atomics wait needs a shared buffer");
        }
        // Step 4: ValidateAtomicAccess.
        let byte_index = self.atomic_index(index, length, &record)?;
        // Steps 5-6: ToBigInt64 / ToInt32 of value; the comparison uses raw element bytes.
        let v = self.element_coerce(record.kind, value)?;
        let expected = record.kind.bytes(&v, true);
        // Steps 7-8: q = ToNumber(timeout); NaN → +∞; t = max(q, 0).
        let q = self.to_number(timeout)?;
        let t = if q.is_nan() {
            f64::INFINITY
        } else {
            q.max(0.0)
        };
        // Step 9: a sync wait needs AgentCanSuspend().
        if !asynchronous && !self.atomics.can_block {
            return self.throw_error("TypeError", "Atomics.wait cannot block this agent");
        }
        let block = store.borrow().bytes.block().cloned().unwrap();
        let result = |e: &mut Self, sync: bool, value: Value| -> Result<Value> {
            let object = e.new_object(Some(e.intrinsics.as_ref().unwrap().object))?;
            e.protect(&Value::Object(object))?;
            e.put_data(object, "async", Value::Bool(!sync))?;
            e.put_data(object, "value", value)?;
            Ok(Value::Object(object))
        };
        let size = record.kind.size();
        // Steps 10-16 run in the WaiterList critical section (the block's lock).
        let mut state = block.lock();
        // Steps 13-14: compare the current value.
        let end = byte_index + size;
        let equal = end <= state.bytes.len() && state.bytes[byte_index..end] == expected[..];
        if !equal {
            drop(state);
            let s = Value::String("not-equal".into());
            return if asynchronous {
                result(self, true, s)
            } else {
                Ok(s)
            };
        }
        let id = SharedBlock::next_waiter_id();
        if !asynchronous {
            // An unbounded wait that no other agent could ever end would just hang.
            if !t.is_finite() && !self.has_peer_agents() {
                return Err(ScriptError::Unsupported(
                    "Atomics.wait without a timeout in a single agent",
                ));
            }
            // Steps 16-17: AddWaiter, then SuspendThisAgent until notified or timed out.
            state.waiters.push(SharedWaiter {
                id,
                byte_index,
                kind: WaiterKind::Sync,
            });
            let deadline = t
                .is_finite()
                .then(|| Instant::now() + Duration::from_secs_f64(t.min(1e9) / 1000.0));
            loop {
                if !state.waiters.iter().any(|w| w.id == id) {
                    return Ok(Value::String("ok".into()));
                }
                match deadline {
                    Some(deadline) => {
                        let now = Instant::now();
                        if now >= deadline {
                            // Step 18-19: still in the list → remove it, "timed-out".
                            state.waiters.retain(|w| w.id != id);
                            return Ok(Value::String("timed-out".into()));
                        }
                        state = block
                            .wake
                            .wait_timeout(state, deadline - now)
                            .unwrap_or_else(|e| e.into_inner())
                            .0;
                    }
                    None => {
                        state = block.wake.wait(state).unwrap_or_else(|e| e.into_inner());
                    }
                }
            }
        }
        // Step 15: t = 0 → a synchronous "timed-out" result.
        if t == 0.0 {
            drop(state);
            return result(self, true, Value::String("timed-out".into()));
        }
        state.waiters.push(SharedWaiter {
            id,
            byte_index,
            kind: WaiterKind::Async(self.atomics.mailbox.clone()),
        });
        drop(state);
        let promise_ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let capability = self.capability(promise_ctor);
        let capability = match capability {
            Ok(c) => c,
            Err(err) => {
                block.lock().waiters.retain(|w| w.id != id);
                return Err(err);
            }
        };
        let resolve = self.runtime.root(capability.resolve.id().unwrap())?;
        self.atomics.waiters.push(Waiter { id, block, resolve });
        // Step 18: EnqueueAtomicsWaitAsyncTimeoutJob for a finite timeout.
        if t.is_finite() {
            self.add_host_job(
                Duration::from_secs_f64(t.min(1e9) / 1000.0),
                HostJob::WaitTimeout(id),
            );
        }
        result(self, false, capability.promise)
    }

    /// Atomics.notify ( typedArray, index, count ) (§25.4.11).
    fn atomics_notify(&mut self, typed: Value, index: Value, count: Value) -> Result<Value> {
        // Step 1: ValidateAtomicAccessOnIntegerTypedArray(typedArray, index, true).
        let access = self.atomic_access(typed, index, true, false)?;
        // Steps 2-3: count undefined → +∞, else max(ToIntegerOrInfinity(count), 0).
        let c = if count == Value::Undefined {
            f64::INFINITY
        } else {
            self.integer(count)?.max(0.0)
        };
        // Step 5: a non-shared buffer has no waiters.
        let Some(block) = access.store.borrow().bytes.block().cloned() else {
            return Ok(Value::number(0.0));
        };
        // Steps 6-11 in the WaiterList critical section: RemoveWaiters, then NotifyWaiter.
        let mut n = 0usize;
        let mut woke = false;
        {
            let mut state = block.lock();
            let mut at = 0;
            while at < state.waiters.len() && (n as f64) < c {
                if state.waiters[at].byte_index == access.byte_index {
                    let w = state.waiters.remove(at);
                    match w.kind {
                        WaiterKind::Sync => woke = true,
                        // EnqueueResolveInAgentJob; the waiter's timeout job finds it gone.
                        WaiterKind::Async(mailbox) => mailbox.post(Event::Notified(w.id)),
                    }
                    n += 1;
                } else {
                    at += 1;
                }
            }
        }
        if woke {
            block.wake.notify_all();
        }
        Ok(Value::number(n as f64))
    }
}
