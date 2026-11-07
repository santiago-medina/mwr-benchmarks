# Async functions and await

Async calls start synchronously and return an intrinsic Promise. Await suspends even for primitive values; continuations use the same FIFO microtask queue as Promise reactions. A compact instruction program reuses the synchronous evaluator for await-free expressions and existing property operations. It saves references, callees, receivers and operands before suspension so effects are never replayed.

Activation registers live in hidden lexical bindings, traced through the existing VM graph. Catch/finally handlers preserve pending returns and exceptions across further awaits. Finishing clears registers and the activation slot. Registers can retain intermediate values until completion. Suspended host invocations hold no borrowed Servo context.

Supported: async declarations, expressions, arrows, object methods, await expressions, lexical blocks, conditionals, return/throw, catch/finally, awaited operands and call arguments, object/array literals. Await uses intrinsic PromiseResolve and internal reactions, bypassing overridden then methods. Async functions are not constructible.

Explicit limits: strict-script subset; no await inside for-of, for-await, async generators, destructuring/default parameters, spread or dynamic Function/AsyncFunction constructors. Unsupported programs are reported as unsupported, not conformant.

Validation: 149 debug and 149 release contracts, including 11 dedicated async contracts under both arena models with allocation GC stress and final zero-object/root/charge checks. The pinned Test262 async profile selects 249 original cases: 40 passed, 209 unsupported, zero failed. All 40 passes also pass Node 24.22.1 with unmodified official sta.js/assert.js. Existing gates retain 1424 object, 1010 array and 125 Promise passes; 42 pre-existing object failures remain visible. See conformance/results-async and required-pass-async.json.

The ten executable contract program literals also pass Node with the unmodified official harness; compare_async_contracts.py extracts their actual Rust test bodies to prevent a mistaken local oracle. This cross-check corrected computed-key coercion: the key value is evaluated before an awaited RHS, but conversion to a property key happens at assignment. Native network integration and its distinct tested commit are recorded in FETCH.md and native-fetch-proof.json.
