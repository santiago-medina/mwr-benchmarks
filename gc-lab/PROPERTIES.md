# Own-property storage and in-place writes (gap 20 side work)

## Problem

Gap 20 (STRINGS.md) made the RegExp `property-escapes` and `CharacterClassEscapes`
cases reachable. They time out: the harness `buildString` (regExpUtils.js) writes
up to 1.1M array elements, and every own-property write cost O(number of properties).

- `Data.properties` was a `Vec<(Key, Property)>`: each lookup and insert was a linear scan.
- Every [[DefineOwnProperty]] cloned the whole `Data` (all properties), applied the
  change and stored it back with `replace_data`. That unlinks every old GC edge and
  links every new one, so each write cost O(n) allocations plus O(n) edge updates.
- The same clone-and-replace was in `delete`, and in `array_define` for `length`
  (every `push` sets `length`, every `pop` shrinks it).

## What the current code constrains

- [[OwnPropertyKeys]] (§10.1.11.1) order: integer indices ascending, then strings in
  creation order, then symbols in creation order. The caller sorts the indices, so
  the store only has to keep insertion order.
- GC edges: the runtime keeps a multiset of edges per object (`Runtime::link` and
  `unlink`). Any store change has to keep that multiset equal to `Data::edges()`.
  `replace_data` did this by linking every edge again and unlinking every old one.
- Many builtins still use `self.data(id)?.clone()` plus `replace_data` for non-property
  fields. Those paths stay correct; they are only slower.

## Options

1. **Keep the Vec, only stop cloning.** The writes become in place, but the lookup
   is still O(n), so `buildString` stays quadratic.
2. **Insertion-ordered entries + hash index (chosen).** `PropertyMap` keeps
   `Vec<Option<(Key, Property)>>` with tombstones for deletes, plus a
   `HashMap<Key, usize>` built once an object has more than 8 live keys. It compacts
   when tombstones outnumber live entries. Small objects keep the cheap linear scan.
   V8 and SpiderMonkey use the same split (a linear "fast" layout for small objects,
   a dictionary for large ones), without hidden classes.
3. **Shapes / hidden classes plus dense element storage.** This is the real answer for
   performance, but it changes the object model. It belongs to roadmap B (the
   bytecode VM design), not to a string gap.

## Design (as implemented)

- `src/script/props.rs`: `PropertyMap` with `get`, `get_mut`, `insert` (replace in
  place or append), `push`, `remove`, `retain`, `len` and ordered `iter`.
- `Data::merged(key, desc)`: ValidateAndApplyPropertyDescriptor (§10.1.6.3). It
  returns the resulting `Property` without storing it.
- `Engine::define_in_place(id, key, desc)`: links the new property's edges, stores the
  property, then unlinks the old property's edges. The multiset invariant holds and
  no `Data` is cloned. When linking fails, the edges already linked are unlinked and
  nothing is stored.
- `Engine::remove_in_place(id, key)`: removes the property, then unlinks its edges.
  `delete` uses it. A mapped-arguments slot cleared by `delete` (§10.4.4.5) also
  unlinks its environment edge.
- `array_define` (§10.4.2.1, ArraySetLength §10.4.2.4):
  - Index keys: `define_in_place`, then update `length` in place.
  - `length` with newLen >= oldLen: `define_in_place` (step 12-13: OrdinaryDefineOwnProperty).
  - `length` with newLen < oldLen: steps 14-20, in place. Indices >= newLen are deleted in
    descending order. When the range is shorter than the property list (`pop`,
    `length -= 1`), they are found by probing the range; otherwise by scanning the
    properties. Deletion stops at the first non-configurable element.
  - Other keys: `define_in_place`.
- `Key::index()` (CanonicalNumericIndexString for array indices) no longer allocates
  a `String` per call.
- `clone_iteration` (CreatePerIterationEnvironment, §14.7.4.4) builds its environment
  with `let_environment(parent, names)` and no longer synthesizes `Stmt::Let`s for
  `new_environment`.

## Migration with no lost passes

The store keeps the same order and the same edge invariant, so the change is
transparent to callers. Clone-and-replace callers still work. Checks: `cargo test`
(dual_heap and ordinary-object GC budget tests included), the 4 gated profiles, and
the full test262 checkpoint 20 compared with full26.

## Results (release `js_check`, wall time, same machine)

| probe | HEAD 869d7fd | after |
|---|---|---|
| q8000 `a[i] = i` x 8000 | 4543 ms | 38-53 ms |
| q8000 `b.push(i)` x 8000 | 8554 ms | 54-82 ms |
| q8000 `new Array(n)` then fill | 5093 ms | 36-50 ms |
| pe2 push x 17325 | 50266 ms | 122-169 ms |
| pe2 `slice` 5000 | 1502 ms | 6-9 ms |
| pop x 8000 | > 300 s (killed) | 96-143 ms |
| delete x 8000 | (not run) | 88-118 ms |

callgrind, `buildString` over 60001 code points (bs60k.js): 3.761 G instructions before
`let_environment`, 3.671 G after (-2.4%).

## What is left (roadmap B)

After these changes, the timeouts are interpreter throughput. `buildString` costs about
61k instructions per loop iteration. The time goes to string-keyed SipHash
`HashMap`/`HashSet` environments (six maps per environment, allocated per iteration
for `let`), a GC allocation per environment, and the AST walk itself. The
CharacterClassEscapes cases pass alone in about 9 s against the 10 s budget. Fixing that
means resolved binding slots and dense elements, the work of the bytecode VM design
note, and not more patches to the tree-walker.
