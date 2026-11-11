# Multiple agents: shared data blocks, cross-agent waiter lists, test262 `$262.agent`

Spec: ECMA-262 §9.7 (Agents), §9.8 (Agent Clusters), §25.2 (SharedArrayBuffer), §25.4.1
(WaiterList critical sections, §25.4.3.14 DoWait, §25.4.11 Atomics.notify), §29 (memory model).
Harness contract: test262 INTERPRETING.md "`agent`". Extends SHARED_MEMORY.md ("Deferred:
multiple agents").

## Goal and gap

The 109 `built-ins/Atomics/{wait,waitAsync,notify}` cases are counted Unsupported because
`$262.agent.start`/`broadcast` are missing. They need real concurrency. The main agent
busy-spins in `$262.agent.waitUntil` (an `Atomics.load` loop) while the other agents run,
and `Atomics.wait` must block one agent until another notifies it. The same machinery is what
Servo's dedicated workers need for `SharedArrayBuffer` (roadmap D), so it is built as engine
code and not as a test-only fake.

## Options considered

1. **Simulated agents on one thread** (cooperative scheduling between several `Engine`s).
   Rejected: a synchronous `Atomics.wait` or a spin loop in one agent would have to suspend the
   engine in the middle of native code. The interpreter has no general way to do that: only
   generator and async frames are resumable. It would also not be reusable for workers.
2. **One OS thread per agent, one `Engine` (one heap, one GC) per agent, sharing only the
   byte blocks of SharedArrayBuffers.** Chosen. This is the model the spec describes (an agent
   cluster with shared memory and separate agents) and what SpiderMonkey/V8 do: a
   reference-counted raw buffer outside the GC heap, referenced by one SAB object per agent.
3. One shared heap with a global lock. Rejected: it rewrites the GC (`Rc` everywhere) for no
   test or web benefit.

## What the current code constrains

- `Engine` is `!Send`: `Rc`, `RefCell`, `ExternalRoot`. That is fine. Each agent builds its
  own `Engine` *inside* its thread, so only `Send` data crosses threads: the source text, the
  `Arc` blocks, mailboxes and report strings.
- Buffer storage is `Store = Rc<RefCell<Buffer>>` with `bytes: Vec<u8>`, and about 60 call
  sites index `bytes` directly. That cannot be shared.

## Design

### Data blocks (`buffers.rs`)

`Buffer.bytes` becomes `Bytes`:

```rust
enum Bytes { Local(Vec<u8>), Shared(Arc<SharedBlock>) }
struct SharedBlock { state: Mutex<BlockState>, wake: Condvar }
struct BlockState { bytes: Vec<u8>, waiters: Vec<SharedWaiter> }
```

- Callers go through `Buffer::len`, `with(|&[u8]|)` and `with_mut(|&mut Vec<u8>|)`. For a
  local buffer these are a match on the enum; for a shared one, a short lock.
- No JS code runs under a lock.
- Copies between two stores read into a temporary `Vec` first, then write. That avoids a
  double lock when two SAB objects in one agent alias the same block.
- **Memory model.** Every shared access, plain or atomic, is serialised by the block's mutex,
  so only sequentially consistent executions occur. The corrected model (Watt et al. 2020) and
  §29 permit them. The atomic read-modify-write operations (§25.4.3-§25.4.15) are one critical
  section each. Production engines do not do this: Boa (`Arc<[AtomicU8]>`), wasmtime, V8 and
  SpiderMonkey use lock-free loads and stores. A mutex is the simplest safe-Rust choice; a
  lock-free byte store is the upgrade path if the shared-path benchmark asks for it.
- **Tearing.** It is allowed by the spec, but never shows up here. That is a valid
  (stronger) choice.
- **Growable SABs** (§25.2.5.3). The length lives in the shared `Vec`, so a `grow` in one agent
  is visible to every agent (ArrayBufferByteLength(seq-cst) reads it under the lock).
  `maxByteLength` is fixed at creation and is copied into every agent's `Buffer`. The block
  reserves `maxByteLength` of capacity at creation, so `grow` never reallocates. That is the
  prior-art invariant in V8, SpiderMonkey and wasmtime.
- **Host allocation budget.** Each `Buffer` records the bytes it charged to its own engine
  (`accounted`), which is the byte length, not the reserved capacity. A growable block's
  reservation is bounded by `MAX_BYTES` (16 MiB). A grow done in another agent is charged only
  to that agent. A SAB object received from another agent charges nothing until it grows the
  block itself.
- **Lifetime.** The block lives while any agent's SAB object holds its `Arc`. It is outside
  every GC heap, so no cross-heap tracing is needed.

### Waiter lists (`atomics.rs`)

The WaiterList of §25.4.1 is `BlockState.waiters`. The critical section
(EnterCriticalSection/LeaveCriticalSection) is the block's mutex.

**`SharedWaiter`** holds:
- `id`
- `byte_index`
- `kind`: either `Sync`, or `Async(Arc<Mailbox>)`, the mailbox of the agent that waits.

**DoWait** (§25.4.3.14) runs steps 10-16 under the lock: it reads the value, compares it and
appends the waiter.
- Sync: SuspendThisAgent loops on `Condvar::wait_timeout` on `wake` until the waiter has left
  the list or the deadline passes, so spurious wakeups are harmless. On timeout it removes
  itself (still under the lock) and returns `"timed-out"`; otherwise it returns `"ok"`.
  `notify_all` on one condition variable per block wakes every sync waiter of the block (a
  known thundering-herd simplification). The upgrade path is one condition variable per
  waiter, like wasmtime's `ParkingSpot`.
- Async: the agent keeps `id → (block, resolve root)` locally. A finite timeout is a local host
  timer (EnqueueAtomicsWaitAsyncTimeoutJob). When the timer fires, it removes the waiter under
  the lock if it is still listed and resolves `"timed-out"`. Otherwise a notification is already
  in its mailbox.

**Atomics.notify** (§25.4.11) removes up to `count` matching waiters in FIFO order under the
lock.
- Sync waiters are woken with `notify_all`.
- For an async waiter, NotifyWaiter → EnqueueResolveInAgentJob pushes `Notified(id)` into the
  waiting agent's mailbox. That agent resolves the promise with `"ok"` on its own thread.

**Infinite waits.** A sync wait with an infinite timeout stays Unsupported only while the
cluster has a single agent (nobody could ever notify, so the case would just hang).

### Agent cluster and mailboxes (`agents.rs`, harness only)

**`Mailbox`** (`Mutex<VecDeque<Event>>` + `Condvar`), one per engine. Its events are
`Notified(waiter id)` and `Broadcast(Arc<SharedBlock>, maximum, number, ack)`.

**Engine host API.** The engine's host jobs gain a source:
- `next_host_job()` returns "now" when the mailbox is not empty.
- `run_host_jobs()` drains it.
- `wait_for_host_event(deadline)` blocks on the mailbox until an event arrives or the deadline
  passes, instead of sleeping.
- `has_pending_waiters()` tells the embedder's loop to keep going while an async waiter could
  still be notified.

**Main agent `$262.agent`:**
- `start(src)` spawns a thread. In it, a child `Engine` takes the parent's step limit, engine
  mode, GC policy and `[[CanBlock]] = true` (a worker-like agent). It evaluates `src` as a
  script and then runs the child event loop. `start` blocks until the top-level script has run
  ("Will block until that agent is running").
- `broadcast(sab[, num])` requires a SharedArrayBuffer (TypeError otherwise). It posts a
  `Broadcast` to every live agent and blocks until each has taken it, or has exited ("blocks
  until all agents have retrieved the message").
- `getReport()` pops the cluster's report queue, or returns `null`. If the queue is empty and
  an agent died with an uncaught error, it throws an `Error` naming that error. The harness
  wrapper loops on `null`, so without this a failed agent shows up as a 10 s timeout instead of
  a failure.
- `sleep`, `monotonicNow`: as before. The epoch is shared by the whole cluster.

**Child agent `$262.agent`:**
- `receiveBroadcast(fn)`
- `report(v)`: ToString(v), queued for the parent.
- `leaving()`
- `sleep`
- `monotonicNow`

**Agent exit.** Before an agent is marked dead or its error is recorded, `release_globals`
removes its remaining waiters from every block (`cancel_host_jobs`). So a later `notify` never
counts a waiter that cannot run. Posting to a dead agent's mailbox is harmless: the `Arc` keeps
it alive, and nobody reads it. Test: `tests/agents.rs` `waiters_of_a_dead_agent_are_purged`.

**Child event loop.** It drains host jobs, mailbox events (a broadcast calls `fn(sab, num)`
with a fresh SAB object over the same block) and microtasks. It exits when `leaving()` was
called and no timer or waiter is pending, or when it has nothing pending and no callback. An
uncaught error ends the agent and is recorded for `getReport`.

**Process exit.** Agent threads are detached. When js_check's main thread returns, the process
exits; js_check's cleanliness checks (live objects and roots) cover the main engine only.

### Not in this step

- Structured clone and `postMessage` between Servo workers (roadmap D). The `Arc<SharedBlock>`
  handle is the piece they will carry.
- Servo's event loop has to drive `wait_for_host_event`/`run_host_jobs` (see SHARED_MEMORY.md).
- A main-thread agent in Servo keeps `[[CanBlock]] = false`.

## Migration without losing passes

1. Switch `Bytes` with only `Local` and `Shared` (single agent) in use. Gate: Atomics,
   SharedArrayBuffer, TypedArray, ArrayBuffer and DataView smokes identical to full52.
2. Move the waiters into the block, add mailboxes and the agent cluster. Gate: an Atomics
   smoke with `--n 0`.
3. Checkpoint full run. Expected: +50 or more of the 109 (kill line: +50 within 3 fix rounds).

**Performance.** For local buffers the only overhead is the enum match per access. Shared
buffers pay one uncontended lock per access. Measurements:
- `copywithin_small` callgrind before and after step 1. Budget: within 2%.
- Shared-path benchmark: the same typed-array loop over a SharedArrayBuffer (single agent,
  uncontended lock) against an ArrayBuffer. Budget: within 3x on the element-access loop.
- Contention micro-benchmark, 2 agents: the main agent spins on `Atomics.load` while a child
  writes in a loop. This one is recorded only; no engine publishes a comparable number.

## Prior art and review

Literature Review (#82), request #405, verdict: "Adopt, with corrections". The architecture
matches prior art: a refcounted block outside the GC heap with one SAB object per agent, a
locked waiter list keyed by block and index, and waitAsync promises resolved by the owning
agent. That is what Boa (`shared.rs`, `futex.rs`), wasmtime (`ParkingSpot`), V8 and
SpiderMonkey do. Two choices have no precedent among the engines checked, though both are
legal: the mutex on plain accesses, and reallocation on grow. The second is fixed (capacity
reserved at creation). The first is kept and measured (see Performance). Report:
`task-2549-solicitud-405-request-literature-check-c/docs/literature-check-sab-multi-agent-2026-11-11.md`.

## Results

**Performance** (2026-11-11, `bench_agents.sh` in the task workspace). Callgrind I refs, VM
engine, release with line tables. Before = HEAD 03a5a3d, after = this change.

| benchmark | before | after | delta |
|---|---|---|---|
| `copywithin_small` | 2,530,710,521 | 2,545,850,127 | +0.60% (budget 2%) |
| `access_ab` (ArrayBuffer loop) | 2,819,996,322 | 2,840,699,349 | +0.73% |
| `access_sab` (SharedArrayBuffer, one agent) | 2,836,708,009 | 2,843,108,938 | +0.23% |

- Shared against local after the change: 2,843,108,938 / 2,840,699,349 = 1.0008x (budget 3x).
  The uncontended lock is lost in the interpreter's per-element cost.
- Contention (`contention.js`, 2 agents: the child does 20000 `Atomics.add` while the main
  agent spins on `Atomics.load`), wall time over 5 runs: 85, 79, 102, 88, 115 ms. Recorded
  only.

**test262**:
- Smoke: built-ins/Atomics 389/389 (109 agent cases were unsupported in full52).
- Buffer regression smoke (600 cases of SharedArrayBuffer/ArrayBuffer/DataView/TypedArray*):
  one failure, already failing in full52.
- Full run: see the checkpoint in IMPLEMENTATION_STATUS.md.
