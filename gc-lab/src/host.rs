//! Simulated DOM/JS ownership using a single shared tracing graph.
//! This does NOT solve tracing across Servo's real DOM and a separate JS GC.
//! Negative payload tags distinguish nodes and callbacks; no actual JS executes.
use crate::{Error, Handle, Heap, Root};

const NODE: i64 = -100;
const CALLBACK: i64 = -200;

/// Return an unrooted temporary. The caller must root it before a safepoint.
/// The node owns its listener and the listener captures the node, forming a cycle.
pub fn create_node_with_listener(heap: &mut Heap) -> Result<Handle, Error> {
    heap.scope(|s| {
        let node = s.try_allocate(NODE)?;
        let node_root = s.root(node)?;
        let callback = s.try_allocate(CALLBACK)?;
        let node = s.resolve(node_root)?;
        s.link(node, callback)?;
        s.link(callback, node)?;
        Ok(node)
    })
}

/// Simulate dispatch, synchronous reentry and a callback returning an object.
/// Each result retains its captured node and (if any) its nested result.
/// Caller owns node_root; operation-owned roots are scoped even on failure.
pub fn dispatch(heap: &mut Heap, node_root: Root, depth: u8) -> Result<Handle, Error> {
    if depth > 32 {
        return Err(Error::ReentryLimit);
    }
    heap.scope(|s| {
        let node = s.resolve(node_root)?;
        if s.value(node)? != NODE {
            return Err(Error::InvalidHandle);
        }
        let callback = s.edge(node, 0)?;
        if s.value(callback)? != CALLBACK {
            return Err(Error::InvalidHandle);
        }
        let callback_root = s.root(callback)?;
        let captured = s.edge(callback, 0)?;
        let capture_root = s.root(captured)?;
        // JS/native reentry may collect while the callback and capture are active.
        s.call(callback_root, |h, _| {
            h.collect();
        });
        let nested_root = if depth > 0 {
            let nested = dispatch(s, capture_root, depth - 1)?;
            Some(s.root(nested)?)
        } else {
            None
        };
        let result = s.try_allocate(i64::from(depth))?;
        let captured = s.resolve(capture_root)?;
        s.link(result, captured)?;
        if let Some(nested) = nested_root {
            let nested = s.resolve(nested)?;
            s.link(result, nested)?;
        }
        Ok(result)
    })
}

/// Removing a listener breaks node -> callback, but a separately rooted callback
/// still retains its captured node through the reverse edge.
pub fn remove_listener(heap: &mut Heap, node: Root) -> Result<(), Error> {
    let node = heap.resolve(node)?;
    if heap.value(node)? != NODE {
        return Err(Error::InvalidHandle);
    }
    heap.clear_edges(node)
}
