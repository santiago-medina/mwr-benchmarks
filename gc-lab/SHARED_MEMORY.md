# Shared memory: SharedArrayBuffer and Atomics (gap 16)

Spec: ECMA-262 §25.2 (SharedArrayBuffer), §25.4 (Atomics), §29 (memory model).
Code: `src/script/buffers.rs` (SAB constructor and prototype), `src/script/atomics.rs`
(Atomics, waiter list, host job queue, test262 `$262` host object).

## Model: one agent per engine

The engine is single-threaded and `#![forbid(unsafe_code)]`. Buffer storage is
`Rc<RefCell<Buffer>>` with `bytes: Vec<u8>`. A SharedArrayBuffer is the same `Buffer` with
`shared: true`:

- `IsSharedArrayBuffer` is that flag. ArrayBuffer methods reject shared receivers and
  SharedArrayBuffer methods reject unshared ones (§25.1.6, §25.2.5 step 3 "If IsSharedArrayBuffer(O) is …").
- With one agent, every memory event is sequentially consistent, so `Atomics.load` and
  `store` and the read-modify-write operations (§25.4.3-§25.4.15) are plain reads and writes of
  `ElementSize` bytes. They are done as a raw `u64` with wrapping arithmetic, and NumericToRawBytes
  / RawBytesToNumeric go through the shared `buffer_elements::Element` codec.
- `Atomics.isLockFree(n)` is true for 1, 2, 4 and 8 (§25.4.7: implementation-defined except 4,
  which must be true).
- Growable SABs (`grow`, §25.2.5.3) only ever increase in length. Because nothing else observes the
  buffer, the length can be updated in place.

### Deferred: multiple agents

test262 multi-agent cases use `$262.agent.start/broadcast/getReport`. Those return
`Unsupported`, so they are counted as unsupported, never as passes (109 cases in
`built-ins/Atomics`). Real agents (Web Workers in Servo, roadmap D) need:

1. A `Send + Sync` byte store for shared buffers, e.g. `Arc<[AtomicU8]>`, or `AtomicU32` words
   with byte-lane access, still in safe Rust. `Rc<RefCell<Vec<u8>>>` cannot cross threads.
2. Each agent's engine on its own thread, with a shared waiter list (`Mutex` + `Condvar`) that
   implements the §25.4.1 WaiterList critical section.
3. Race semantics for non-atomic accesses (§29): with safe Rust atomics per byte, tearing is
   allowed by the spec, so relaxed per-byte access is enough.

That is a GC and threading change, so it gets its own design note before it starts.

## [[CanBlock]] and Atomics.wait

`AgentCanSuspend()` is false by default (`AtomicsState::can_block`), which is the value for a
browser main thread. With it false, synchronous `Atomics.wait` throws a TypeError (DoWait step 10).
Embedders call `Engine::set_can_block(true)`. `js_check --can-block` does it, and
`conformance/run.py` passes the flag for tests flagged `CanBlockIsTrue`.

With one agent no other agent can notify, so a blocking wait:
- returns `"not-equal"` when the value differs,
- otherwise sleeps the thread for the timeout and returns `"timed-out"`,
- with an infinite timeout would block forever, so it returns `Unsupported` instead of hanging.

## Atomics.waitAsync and the host job queue

`DoWait(async)` (§25.4.3.14) creates a PromiseCapability. The waiter record roots its resolve
function with an `ExternalRoot`, so the GC keeps the promise alive while it waits. If the timeout is
finite, `EnqueueAtomicsWaitAsyncTimeoutJob` becomes a timer in `AtomicsState::timers`.
`Atomics.notify` removes up to `count` matching waiters (same buffer store, same byte index, FIFO)
and enqueues `Resolve(.., "ok")` jobs (EnqueueResolveInAgentJob, §25.4.1.7).

These are **host jobs**, not microtasks. The embedder drains them:

```rust
while let Some(deadline) = engine.next_host_job() {
    sleep_until(deadline);
    engine.run_host_jobs()?;   // runs due timers and resolve jobs
    engine.run_microtasks()?;  // then a microtask checkpoint
}
```

`js_check` runs this loop after the script, together with FinalizationRegistry cleanup.
`release_globals` cancels pending host jobs and drops their roots.

**Servo bridge (roadmap D):** the Servo event loop has to call `next_host_job()` /
`run_host_jobs()` as a task source, just as `run_finalization_cleanup` is wired. Until it does, a
`waitAsync` with a timeout never settles on the Servo path. A blocking `sleep` must not be used
there: the deadline should be fed to Servo's timer scheduler.

## Test262 host object (harness only)

Only with `test_harness` on, the engine defines:

- `$262`, per test262 INTERPRETING.md:
  - `detachArrayBuffer`: DetachArrayBuffer; a TypeError on a SAB.
  - `gc`: a full collection.
  - `global`.
  - `createRealm` and `evalScript`: Unsupported, because there is a single realm.
  - `agent`: `start` and `broadcast` are Unsupported, `getReport` returns null, `sleep` blocks the
    thread, `monotonicNow` returns elapsed milliseconds.
- `setTimeout(fn, ms)`: queues a host job. `atomicsHelper.js` falls back to a busy promise loop
  without it, which burns the step budget.

Neither is part of the web-facing global. Servo provides its own `setTimeout` (HTML timers).

## Order of checks in the constructors

AllocateArrayBuffer / AllocateSharedArrayBuffer (§25.1.3.1, §25.2.2.1) check
byteLength > maxByteLength (step 2) before OrdinaryCreateFromConstructor (step 3, the `prototype`
Get). The host allocation limit (CreateByteDataBlock's RangeError) only applies after it. test262
`data-allocation-after-object-creation.js` checks that order.

## Results (smoke, 2026-11-09)

- `built-ins/SharedArrayBuffer`: 103/104. The remaining case is proto-from-ctor-realm (one realm).
- `built-ins/Atomics`: in full22 every case was failing or unsupported. Now 271 pass (264 in the
  smoke, plus 7 CanBlockIsTrue cases that the smoke runs without `--can-block`). 109 are
  unsupported because they need agents, and 9 fail because they need immutable ArrayBuffers (a
  separate gap).
