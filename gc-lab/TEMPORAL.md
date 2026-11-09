# Temporal (design note)

Status: in progress (gap 11, 2026-11-09). Spec: ECMA-262 + Temporal proposal (stage 4 text as vendored by test262), §§ cited in `src/script/temporal.rs`.

## Decision: the arithmetic comes from `temporal_rs`, the JS layer is ours

Temporal is calendars, the tz database and RFC 9557 parsing. Per the engine rules it comes from a vetted crate, not hand-written. Santiago approved `temporal_rs =0.2.6` on 2026-11-09 (proposal `TEMPORAL_DEPENDENCY_PROPOSAL.md` in the task workspace).

The audit by Rust Unsafe Security Auditor (#92), collab #311, found it **SOUND for this use**. There are 513 `unsafe` sites across 17 transitive crates, but none in the crates that see JS input (temporal_rs, ixdtf, timezone_provider, tzif, calendrical_calculations, jiff-tzdb). There are no open advisories. Lockfile floors: yoke ≥ 0.8.3, bytes ≥ 1.11.1, memchr ≥ 2.8.2. The `sys-local` feature (iana-time-zone FFI) is excluded. The residual risk is a panic (DoS): every call into the crate goes through `catch_unwind` and turns a panic into a `RangeError` (see "Panics").

Split of work:

- **Ours (safe Rust, no deps):**
  - constructors, prototypes, getters
  - argument coercion and options reading (`GetOptionsObject`, `GetTemporalOverflowOption`, `GetDifferenceSettings`, `GetRoundingIncrementOption`, …) in the spec's observable order
  - property bags (`PrepareCalendarFields`, `ToTemporalDurationRecord`)
  - receiver brand checks
  - error types
  
  test262 checks the observable order of `Get`/`ToString` calls heavily, so this layer follows the spec steps one by one.
- **The crate's:**
  - ISO/calendar arithmetic, balancing and rounding
  - parsing and formatting of ISO strings
  - time zone resolution (compiled tzdb)

## Build layout: a second manifest, the main crate stays dependency-free

An *optional* dependency does not keep the default build offline. `cargo metadata --offline` with an empty registry fails with "no matching package named `temporal_rs`", even with the feature off. The resolver needs the index for the lockfile. That would break:

- every CI step using `--offline`
- Servo's vendored workspace, which copies `gc-lab/Cargo.toml` + `src/` as a path dependency and builds `--offline` against its own lockfile

Vendoring the crates into git would add 27 MB (icu_calendar alone is 11 MB). We rejected that.

Chosen layout:

- `gc-lab/Cargo.toml` is unchanged: zero dependencies. It declares `cfg(feature = "temporal")` as an expected cfg, so the lint stays quiet.
- `gc-lab/temporal/Cargo.toml` is a second package with its own `Cargo.lock` (committed, floors above) and its own `target/`:
  - Its `[lib]` path is `../src/lib.rs` and its only bin is `js_check` (`../src/bin/js_check.rs`).
  - It depends on `temporal_rs` and turns on feature `temporal` by default.
  - The same sources compile with the Temporal module included.
- Engine code touching Temporal is behind `#[cfg(feature = "temporal")]`:
  - the module itself
  - the `Data.temporal` slot
  - the bootstrap call and the dispatch arms
  
  Without the feature, `Temporal` stays undefined, as before.
- CI adds one online step: build `gc-lab/temporal` and run the Temporal smoke/profile with that `js_check`. The full test262 profile uses that binary when it is given via `--binary`.

Migration later: roadmap C needs a public native-extension API (host objects with native data, native functions) for Servo's WebIDL bindings. When that API exists, the Temporal module can move out into its own crate on top of it, with no change in behaviour.

## Object model

- One slot, `Data.temporal: Option<Rc<TemporalSlot>>`. `TemporalSlot` is an enum over the crate's value types: `Duration`, `PlainDate`, `PlainTime`, `PlainDateTime`, `PlainYearMonth`, `PlainMonthDay`, `Instant`, `ZonedDateTime`.
  - The values are immutable. `Rc` keeps the existing `Data` clone-on-write pattern cheap.
  - They hold no JS references, so `edges()` does not change.
- Brand check: the variant must match, otherwise `TypeError`. This is `RequireInternalSlot`.
- Builtins are dispatched by name (`Temporal.PlainDate.prototype.add`), like Date and RegExp. Constructors are `Temporal.<Type>`, and `GetPrototypeFromConstructor` reads `newTarget.prototype` and falls back to the intrinsic.
- `Temporal` is a plain namespace object:
  - its members are writable, configurable and non-enumerable
  - `@@toStringTag` is `"Temporal"`
  - `Temporal.Now` is a namespace too

## Panics

`temporal_rs` has about 600 `unwrap()` calls that nobody has triaged. Each crate call runs under `std::panic::catch_unwind(AssertUnwindSafe(..))`:

- The closure only touches crate values, never the engine heap, so a panic cannot leave the heap half-mutated.
- A caught panic becomes `RangeError: Temporal internal error`.
- A test262 case that hits one is logged as a crate bug to report upstream.

`catch_unwind` needs `panic = "unwind"`. That is the default, and gc-lab does not set `abort`.

## Order of work (each step is one smoke-gated feature)

1. Infrastructure + `Temporal.Duration` (540 cases). Every other type's `add`/`subtract`/`until` returns or accepts durations.
2. `PlainDate` (654), `PlainTime` (493), `PlainDateTime` (776).
3. `PlainYearMonth` (511), `PlainMonthDay` (199).
4. `Instant` (467), `ZonedDateTime` (902), `Now` (66).
5. `intl402/Temporal` (2040): only what does not need Intl formatting (calendars other than ISO come with the crate's compiled data).

Kill line: 3 fix rounds on the same sub-bucket (one type's directory) without progress means a narrower survey of that directory's failure reasons before more code.

## Status after step 4 (2026-11-09)

All eight types and `Temporal.Now` are implemented. The `built-ins/Temporal` smoke run passes 4613/4613.

- `Temporal.Now`: the host hooks (`SystemHost`) take the clock from `std::time::SystemTime`. The system time zone is UTC, because ECMA-262 SystemTimeZoneIdentifier (in the Date section) is implementation-defined. The crate's `sys-local` feature stays off (unaudited FFI). A Servo embedding that wants the user's zone passes it through the host later.
- Overflowing years: property-bag years outside ±1e9 are mapped into the same position of the 400-year Gregorian cycle near ±1e9 (`clamp_year`), so the leap-year checks used for overflow (`PlainMonthDay.from` with a huge `year`) keep their parity. The value is out of range anyway and is rejected wherever the year is used as a date.
- `intl402/Temporal` is not started. Most of it needs Intl formatting (`toLocaleString`) or non-ISO calendar arithmetic through `Intl.DateTimeFormat`.
