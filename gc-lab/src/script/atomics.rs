//! Atomics (ECMA-262 §25.4), the waiter lists behind Atomics.wait / waitAsync / notify, and
//! the test262 host object `$262` (harness only). Design: SHARED_MEMORY.md.
//!
//! One agent per engine: shared bytes are the `Vec<u8>` behind a `Store`, so every access
//! is trivially sequentially consistent. A sync wait can only time out (no other agent can
//! notify), and async waiters are settled through host jobs that the embedder drains with
//! `run_host_jobs` (js_check does; the Servo bridge must too, see SHARED_MEMORY.md).
use super::{
    buffer_elements::Element,
    buffers::Store,
    objects::{Descriptor, Key},
    vm::{Engine, ScriptError, Value},
};
use crate::dual::{ExternalRoot, Id};
use std::{
    collections::HashMap,
    rc::Rc,
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

/// A waiter of an async wait (§25.4.3.14 Waiter Record): the promise's resolve function and
/// the timer that settles it with "timed-out".
struct Waiter {
    id: u64,
    store: Store,
    byte_index: usize,
    resolve: ExternalRoot,
}

enum HostJob {
    /// `setTimeout(callback)` from the test harness host.
    Callback(ExternalRoot),
    /// EnqueueResolveInAgentJob (§25.4.3.12): resolve a notified waiter's promise with "ok".
    Resolve(ExternalRoot, &'static str),
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
    epoch: Instant,
    waiters: Vec<Waiter>,
    timers: Vec<Timer>,
    next: u64,
}
impl Default for AtomicsState {
    fn default() -> Self {
        Self {
            can_block: false,
            epoch: Instant::now(),
            waiters: vec![],
            timers: vec![],
            next: 0,
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
    /// The earliest pending host job (timer or async-waiter settlement), if any.
    pub fn next_host_job(&self) -> Option<Instant> {
        self.atomics.timers.iter().map(|t| t.deadline).min()
    }
    /// Run every host job whose deadline has passed, in deadline then FIFO order.
    pub fn run_host_jobs(&mut self) -> Result<()> {
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
        let mut first = Ok(());
        for timer in due {
            self.steps_for_external_event();
            let result = match timer.job {
                HostJob::Callback(root) => self.run_rooted(root, None),
                HostJob::Resolve(root, value) => self.run_rooted(root, Some(value)),
                HostJob::WaitTimeout(id) => {
                    match self.atomics.waiters.iter().position(|w| w.id == id) {
                        Some(at) => {
                            let w = self.atomics.waiters.remove(at);
                            self.run_rooted(w.resolve, Some("timed-out"))
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
            if let HostJob::Callback(root) | HostJob::Resolve(root, _) = t.job {
                let _ = self.runtime.unroot(root);
            }
        }
        for w in std::mem::take(&mut self.atomics.waiters) {
            let _ = self.runtime.unroot(w.resolve);
        }
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
        for (method, length) in [
            ("start", 1),
            ("broadcast", 2),
            ("getReport", 0),
            ("sleep", 1),
            ("monotonicNow", 0),
        ] {
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
                Self::detach_buffer(&mut buffer.borrow_mut());
                Ok(Value::Null)
            }
            "$262.gc" => {
                self.collect();
                Ok(Value::Undefined)
            }
            "$262.createRealm" => Err(ScriptError::Unsupported("$262.createRealm (one realm)")),
            "$262.evalScript" => Err(ScriptError::Unsupported("$262.evalScript")),
            "$262.agent.start" | "$262.agent.broadcast" => Err(ScriptError::Unsupported(
                "test262 agents (one agent per engine)",
            )),
            "$262.agent.getReport" => Ok(Value::Null),
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
                let access = self.atomic_access(arg(0), arg(1), false)?;
                self.revalidate_atomic_access(&access)?;
                let size = access.kind.size();
                let b = access.store.borrow();
                Ok(access
                    .kind
                    .read(&b.bytes[access.byte_index..access.byte_index + size], true))
            }
            "Atomics.store" => {
                // §25.4.12 Atomics.store ( typedArray, index, value ).
                let access = self.atomic_access(arg(0), arg(1), false)?;
                let v = self.atomic_operand(access.kind, arg(2))?;
                self.revalidate_atomic_access(&access)?;
                let bytes = access.kind.bytes(&v, true);
                let mut b = access.store.borrow_mut();
                b.bytes[access.byte_index..access.byte_index + bytes.len()].copy_from_slice(&bytes);
                Ok(v)
            }
            "Atomics.compareExchange" => {
                // §25.4.5 Atomics.compareExchange ( typedArray, index, expectedValue,
                // replacementValue ).
                let access = self.atomic_access(arg(0), arg(1), false)?;
                let expected = self.atomic_operand(access.kind, arg(2))?;
                let replacement = self.atomic_operand(access.kind, arg(3))?;
                self.revalidate_atomic_access(&access)?;
                let expected = access.kind.bytes(&expected, true);
                let replacement = access.kind.bytes(&replacement, true);
                let range = access.byte_index..access.byte_index + access.kind.size();
                let mut b = access.store.borrow_mut();
                let old = access.kind.read(&b.bytes[range.clone()], true);
                if b.bytes[range.clone()] == expected[..] {
                    b.bytes[range].copy_from_slice(&replacement);
                }
                Ok(old)
            }
            "Atomics.add" | "Atomics.and" | "Atomics.exchange" | "Atomics.or" | "Atomics.sub"
            | "Atomics.xor" => {
                // AtomicReadModifyWrite (§25.4.3.17).
                let access = self.atomic_access(arg(0), arg(1), false)?;
                let v = self.atomic_operand(access.kind, arg(2))?;
                self.revalidate_atomic_access(&access)?;
                let operand = raw(&access.kind.bytes(&v, true));
                let size = access.kind.size();
                let range = access.byte_index..access.byte_index + size;
                let mut b = access.store.borrow_mut();
                let old_bytes = b.bytes[range.clone()].to_vec();
                let old = raw(&old_bytes);
                let new = match name {
                    "Atomics.add" => old.wrapping_add(operand),
                    "Atomics.sub" => old.wrapping_sub(operand),
                    "Atomics.and" => old & operand,
                    "Atomics.or" => old | operand,
                    "Atomics.xor" => old ^ operand,
                    _ => operand,
                };
                for (i, byte) in b.bytes[range].iter_mut().enumerate() {
                    *byte = (new >> (i * 8)) as u8;
                }
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
    fn atomic_access(&mut self, typed: Value, index: Value, waitable: bool) -> Result<Access> {
        let (id, record, length) = self.typed_record(typed)?;
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
        // Steps 13-14: compare the current value (no other agent can change it meanwhile).
        let current = {
            let b = store.borrow();
            let end = byte_index + record.kind.size();
            if end > b.bytes.len() {
                None
            } else {
                Some(b.bytes[byte_index..end].to_vec())
            }
        };
        let equal = current.as_deref() == Some(&expected[..]);
        let result = |e: &mut Self, sync: bool, value: Value| -> Result<Value> {
            let object = e.new_object(Some(e.intrinsics.as_ref().unwrap().object))?;
            e.protect(&Value::Object(object))?;
            e.put_data(object, "async", Value::Bool(!sync))?;
            e.put_data(object, "value", value)?;
            Ok(Value::Object(object))
        };
        if !equal {
            let s = Value::String("not-equal".into());
            return if asynchronous {
                result(self, true, s)
            } else {
                Ok(s)
            };
        }
        if !asynchronous {
            // Step 17 SuspendThisAgent: with one agent nobody can notify, so the wait
            // always times out; an unbounded wait could never return.
            if !t.is_finite() {
                return Err(ScriptError::Unsupported(
                    "Atomics.wait without a timeout in a single agent",
                ));
            }
            std::thread::sleep(Duration::from_secs_f64(t.min(10_000.0) / 1000.0));
            return Ok(Value::String("timed-out".into()));
        }
        // Step 15: t = 0 → a synchronous "timed-out" result.
        if t == 0.0 {
            return result(self, true, Value::String("timed-out".into()));
        }
        let promise_ctor = Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]);
        let capability = self.capability(promise_ctor)?;
        let resolve = self.runtime.root(capability.resolve.id().unwrap())?;
        let id = self.atomics.next;
        self.atomics.next += 1;
        self.atomics.waiters.push(Waiter {
            id,
            store,
            byte_index,
            resolve,
        });
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
        let access = self.atomic_access(typed, index, true)?;
        // Steps 2-3: count undefined → +∞, else max(ToIntegerOrInfinity(count), 0).
        let c = if count == Value::Undefined {
            f64::INFINITY
        } else {
            self.integer(count)?.max(0.0)
        };
        // Step 5: a non-shared buffer has no waiters.
        if !access.store.borrow().shared {
            return Ok(Value::number(0.0));
        }
        let mut n = 0usize;
        let mut at = 0;
        while at < self.atomics.waiters.len() && (n as f64) < c {
            let w = &self.atomics.waiters[at];
            if Rc::ptr_eq(&w.store, &access.store) && w.byte_index == access.byte_index {
                let w = self.atomics.waiters.remove(at);
                // NotifyWaiter → EnqueueResolveInAgentJob; the waiter's timeout job is
                // dropped when it finds the waiter gone.
                self.add_host_job(Duration::ZERO, HostJob::Resolve(w.resolve, "ok"));
                n += 1;
            } else {
                at += 1;
            }
        }
        Ok(Value::number(n as f64))
    }
}
