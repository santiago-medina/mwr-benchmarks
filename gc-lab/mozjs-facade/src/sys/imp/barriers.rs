/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! GC barriers (js/public/HeapAPI.h, js/public/RootingAPI.h). Servo's
//! `Heap<T>` and `init_reflector` call them on every store of a GC pointer.
//!
//! They are empty: the C1 keep-alive policy (objects.rs) never frees a cell
//! and reports every cell as a root, the engine's collector is not
//! incremental and never moves objects, so there is no remembered set to
//! update and no incremental mark to preserve. Tracing (E4) replaces the
//! policy; whether it then needs a store barrier is decided in that design
//! note (MOZJS_FACADE.md "GC barriers").

use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;

pub unsafe fn HeapObjectWriteBarriers(
    _objp: *mut *mut root::JSObject,
    _prev: *mut root::JSObject,
    _next: *mut root::JSObject,
) {
}

pub unsafe fn HeapObjectPostWriteBarrier(
    _objp: *mut *mut root::JSObject,
    _prev: *mut root::JSObject,
    _next: *mut root::JSObject,
) {
}

pub unsafe fn HeapStringWriteBarriers(
    _objp: *mut *mut root::JSString,
    _prev: *mut root::JSString,
    _next: *mut root::JSString,
) {
}

pub unsafe fn HeapBigIntWriteBarriers(
    _bip: *mut *mut JS::BigInt,
    _prev: *mut JS::BigInt,
    _next: *mut JS::BigInt,
) {
}

pub unsafe fn HeapScriptWriteBarriers(
    _objp: *mut *mut root::JSScript,
    _prev: *mut root::JSScript,
    _next: *mut root::JSScript,
) {
}

pub unsafe fn HeapValueWriteBarriers(
    _valuep: *mut JS::Value,
    _prev: *const JS::Value,
    _next: *const JS::Value,
) {
}

pub unsafe fn HeapValuePostWriteBarrier(
    _valuep: *mut JS::Value,
    _prev: *const JS::Value,
    _next: *const JS::Value,
) {
}

pub unsafe fn IsIncrementalBarrierNeeded(_cx: *mut root::JSContext) -> bool {
    false
}

pub unsafe fn IncrementalPreWriteBarrier(_obj: *mut root::JSObject) {}

pub unsafe fn IncrementalPreWriteBarrier1(_thing: JS::GCCellPtr) {}

pub unsafe fn PerformIncrementalReadBarrier(_thing: JS::GCCellPtr) {}

/// A reserved-slot store with barriers: the barriers are empty, so it is
/// `JS_SetReservedSlot`.
pub unsafe fn SetNativeObjectReservedSlotWithBarrier(
    obj: *mut root::JSObject,
    slot: usize,
    value: *const JS::Value,
) {
    unsafe { super::objects::JS_SetReservedSlot(obj, slot as u32, value) }
}

// --- Lazy standard classes (jsapi.h) ---
//
// The engine defines every standard class eagerly when it creates a global,
// so a global's resolve hook never has anything left to resolve.

pub unsafe fn JS_ResolveStandardClass(
    _cx: *mut root::JSContext,
    _obj: root::JS::HandleObject,
    _id: root::JS::HandleId,
    resolved: *mut bool,
) -> bool {
    // SAFETY: the caller's out-parameter.
    unsafe { *resolved = false };
    true
}

pub unsafe fn JS_MayResolveStandardClass(
    _names: *const root::JSAtomState,
    _id: root::jsid,
    _maybe_obj: *mut root::JSObject,
) -> bool {
    false
}

pub unsafe fn JS_EnumerateStandardClasses(
    _cx: *mut root::JSContext,
    _obj: root::JS::HandleObject,
) -> bool {
    true
}

pub unsafe fn JS_NewEnumerateStandardClasses(
    _cx: *mut root::JSContext,
    _obj: root::JS::HandleObject,
    _properties: root::JS::MutableHandleIdVector,
    _enumerable_only: bool,
) -> bool {
    true
}
