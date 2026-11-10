# Import attributes and JSON modules

Scope: ECMA-262 ImportAttributes (§16.2.2), the second argument of `import()` (§13.3.10), ModuleRequest Records with attributes (§16.2.1.3), and JSON modules (§16.2.1.8 ParseJSONModule, §16.2.1.7 CreateDefaultExportSyntheticModule). The `import-text` and `import-bytes` proposals are not part of ECMA-262 and stay Unsupported.

## Syntax (§16.2.2)

- `WithClause : with { } | with { WithEntries ,opt }`, `AttributeKey : IdentifierName | StringLiteral`, the value is a StringLiteral. It is parsed by `module_parser.rs` `specifier()` on `import ... from`, bare `import "m"`, `export ... from` and `export * from`.
- Early error: duplicate AttributeKey StringValues (§16.2.2.1).
- `assert { ... }` (the withdrawn syntax) is not accepted: it falls through as an unexpected token, a real SyntaxError.
- `import(spec, options ,opt)` parses into `Expr::ImportCall(spec, Some(options), Evaluation)`. The phase forms `import.source(...)` / `import.defer(...)` take one argument (IMPORT_PHASES.md); a second one is an early SyntaxError ("import phase call takes one argument").

## ModuleRequest Records

`RequestKey { specifier, attributes }` keeps the attributes sorted by key, so ModuleRequestsEqual (§16.2.1.3.1) is `==` and the key can index `[[LoadedModules]]` (`Record.loaded`), the in-flight table and the script `[[LoadedModules]]`. `Module.requests` is deduplicated on (key, phase). The host outbox (`ModuleRequest`) carries the attributes to the embedder.

## Loading

- InnerModuleLoading (§16.2.1.5.1.1 step 2.c.ii.1): before HostLoadImportedModule, each request is checked by AllImportAttributesSupported against HostGetSupportedImportAttributes. The engine host supports « "type" » (the HTML host's list). An unsupported key calls ContinueModuleLoading with a SyntaxError.
- EvaluateImportCall (§13.3.10.2): the specifier and the options are evaluated in order (abrupt → throw); ToString(specifier) abrupt → reject; options not undefined and not an Object → reject with TypeError; Get(options, "with") abrupt → reject; a non-undefined non-Object value → TypeError; EnumerableOwnProperties(key+value) with every value a String (else TypeError); unsupported keys → TypeError. Implemented in `modules.rs` `import_call_attributes`.

## JSON modules

- The host (js_check, and later Servo's module loader) maps `type: "json"` to module type `"json"`; any other type (`"text"`, `"bytes"`, unknown) is a load failure that the engine reports as Unsupported in js_check, so those proposals never count as passes.
- `module_record` with type `"json"` validates the source with the JSON.parse grammar at load time (ParseJSONModule step 1; a syntax error is a SyntaxError during loading, i.e. resolution phase), then builds a Synthetic-like record: no requests, one local export `default` bound to `"\0default"`.
- The value is materialized by JSON.parse when the module environment is created (SetSyntheticModuleExport). This is unobservable compared to parsing at load, because the record has no dependencies and its environment is created before any importer evaluates.
- The module map key is (URL, "json"), so `import a from "./x.json" with {type: "json"}` and a JavaScript import of the same URL are distinct records, as HTML's module map requires.

## Results

- Bucket (dynamic-import, import/import-attributes, module-code/import-attributes): 89/100 pass; the other 11 are import-text (6) and import-bytes (5), Unsupported by design.
- Smoke over dynamic-import, module-code, import, export, import.meta and JSON/parse: 1782 passed, 5 failed (JSON.parse reviver/source cases that already failed), 48 unsupported; 0 lost.
