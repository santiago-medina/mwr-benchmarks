//! A map over a fixed ordered graph, NOT ECMAScript Array.prototype.map.
//! Callbacks return a scalar, may allocate/collect/reenter, but must not release
//! input roots or mutate the input graph. Exceptions propagate via Result.
use crate::{Error, Handle, Heap, Model, Root};

/// Stable tokens can remain local across safepoints, while rooted elsewhere.
pub fn map_stable(
    heap: &mut Heap,
    input: Root,
    mut callback: impl FnMut(&mut Heap, Root) -> Result<i64, Error>,
) -> Result<Handle, Error> {
    assert!(matches!(heap.model, Model::Stable | Model::Indirect));
    heap.scope(|s| {
        let source = s.resolve(input)?;
        let length = s.edges(source)?.len();
        let output = s.allocate(0);
        let _output_root = s.root(output)?;
        for index in 0..length {
            let mapped = s.scope(|inner| {
                let item = inner.edge(source, index)?;
                let item = inner.root(item)?;
                callback(inner, item)
            })?;
            let value = s.allocate(mapped);
            s.link(output, value)?;
        }
        Ok(output)
    })
}

/// Portable path also works when raw tokens are invalidated by compaction.
pub fn map_resolving(
    heap: &mut Heap,
    input: Root,
    mut callback: impl FnMut(&mut Heap, Root) -> Result<i64, Error>,
) -> Result<Handle, Error> {
    heap.scope(|s| {
        let length = s.edges(s.resolve(input)?)?.len();
        let output = s.allocate(0);
        let output_root = s.root(output)?;
        for index in 0..length {
            let mapped = s.scope(|inner| {
                let source = inner.resolve(input)?;
                let item = inner.edge(source, index)?;
                let item = inner.root(item)?;
                callback(inner, item)
            })?;
            let value = s.allocate(mapped);
            let output = s.resolve(output_root)?;
            s.link(output, value)?;
        }
        s.resolve(output_root)
    })
}
