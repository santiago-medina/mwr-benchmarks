# ES modules: linking, evaluation and native loading

Verified milestone: commit `788f5209bcd199a5430e086c1bfa35379810b8f9`, [Actions run 42421045635](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/42421045635), all jobs successful. Full evidence and fingerprints are in native-module-proof.json; native-modules.png shows the actual browser output.

## Core

Module syntax shares the script tokenizer, expression grammar, validation and variable-declaration analysis. Named/default imports, side-effect imports, named/default exports and explicit reexports are supported. Imports resolve to immutable aliases of the exporter's ordinary lexical bindings, with TDZ and live updates. The existing Runtime graph traces alias-to-environment edges; imported values are never copied. Module variables remain outside globalThis and top-level this is undefined.

All reachable environments and function declarations are instantiated before evaluation, so cycles can call hoisted functions. Export resolution follows reexport/import chains and rejects missing or unresolved cyclic exports before executing user code. URL identity deduplicates a dependency shared by several importers.

Evaluation identifies strongly connected components and runs dependency-ready bodies in DFS order. Top-level await uses the existing resumable instruction program and Promise queue, in the already-instantiated module environment. Static await remains asynchronous even when a branch skips the await or an exception precedes it. Cycle backedges, including self imports, do not wait for their own completion. Errors are cached, propagated to importers, and kept alive in traced environment slots. Synchronous failure stops later unvisited branches; independent siblings can proceed while an asynchronous dependency waits.

The core receives ModuleSource records with canonical identities and host-resolved specifiers. It does not guess filesystem or browser URL policy. execute_modules creates a realm and evaluates its reachable graph; module_status and module_binding_string provide typed host observation. Registry contents and pending jobs are discarded on realm release.

## Native Servo

One opt-in `application/x-mwr-module` entry per document is supported, inline or with src. The HTML preparation path supplies actual script fetch options: nonce, integrity, credentials, parser metadata and referrer policy. Module requests use Servo networking with destination Script and CORS mode, not Window.fetch's connect-src route. Descendants resolve against the response's final URL, inherit fetch options and response referrer policy, require successful HTTP status and JavaScript MIME, and decode UTF-8 through Servo's decoder. Source is interpreted by our engine; SpiderMonkey does not compile or evaluate these program bodies.

Downloads are deduplicated before dispatch. Each source listener carries a pipeline ID and plain source/metadata, with managed globals owned through cancelable native leases. Unload removes the session first, aborts requests, removes fetch bookkeeping and detaches listener roots. Late source completion cannot revive a retired VM. Ready evaluation waits until parsing finishes. Native load blockers cover download/link/start; test-only blockers hold screenshots until the final observable result, without introducing unrelated image requests that could retain a retired document.

## Validation

- 163 contracts pass in debug and release, including both GC arena models and allocation stress. Module contracts finish with zero live objects, roots, metadata and charged bytes after realm release.
- The 13 actual executable Rust contract graphs pass Node SourceTextModule with the unchanged official Test262 assertion harness.
- 96 seeded graphs with cycles, self imports, siblings, top-level await and skipped-await branches match Node's observable evaluation order. The same generated files run through module_check; no source rewriting or replacement helpers are used.
- Existing Test262 pass baselines remain: objects 1424, arrays 1010, Promises 125, async 40. The 42 pre-existing object failures remain visible; this is not full ECMAScript conformance.
- Eleven native scenarios pass: application with eight modules, external entry, modules allowed while connect-src is blocked, script-src denial, CORS denial, bad MIME, HTTP error, link error, evaluation error, navigation with source bytes still blocked, and disabled backend. The shared dependency is fetched and evaluated once.
- Navigation proves old session and document collection and ignores an actual source-dispatch call with the retired pipeline ID. Previous applications, shared microtask FIFO, native fetch and the 1500-cycle GC test also pass: 3000 native finalizations, 120 VM objects before/after, two navigations, no manual global release.

## Reproduction and limits

```sh
cargo build --manifest-path gc-lab/Cargo.toml --bin module_check --release --offline
python3 gc-lab/conformance/compare_module_contracts.py --source gc-lab/.test262-source
python3 gc-lab/conformance/differential_modules.py
python3 gc-lab/servo-native/run_modules.py --servo gc-lab/.servo-native/target/debug/servoshell
```

This is the existing strict JavaScript subset with module semantics, not a complete browser engine. Namespace imports/objects, export-star, dynamic import, import.meta, import maps and import attributes/JSON modules are unsupported. The browser loader accepts URL-like specifiers; bare package resolution needs import-map support. Core graphs are bounded to 128 modules and 1 MiB aggregate source, with the existing per-source parser limits. Only one experimental entry is allowed per document, and ordinary type=module scripts continue to use Servo's existing engine.
