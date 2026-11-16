//! The Promise API (js/public/Promise.h) over the engine's own §27.2 abstract
//! operations (`Engine::new_promise`, `resolve_promise`, `perform_promise_then`
//! and friends, PROMISES.md "Embedder API"). Nothing here re-implements a
//! promise algorithm: each entry point converts handles and calls one engine
//! operation. Promise jobs reach the embedder queue through `Host::enqueue_job`
//! (jobs.rs); the rejection tracker is `FacadeHost::promise_rejection_tracker`.

use std::os::raw::{c_char, c_uint};
use std::ptr;

use gc_lab::script::{Engine, JsValue, ObjectId, PromiseStatus, ScriptError};

use super::context::{context, Context};
use super::natives::{
    from_jsval, handle_object, handle_object_or_null, object_ptr, run_parked, to_jsval,
};
use super::objects::{engine_result, object_cell};
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;
use crate::sys::jsval::{JSVal, UndefinedValue};

/// The context and engine id of a promise handle (entry points without a cx).
unsafe fn promise_of<'a>(promise: JS::HandleObject) -> (&'a Context, ObjectId) {
    // SAFETY: a Handle points to a rooted, live object cell.
    let cell = unsafe { object_cell(*promise.ptr) };
    (cell.context(), cell.id)
}

fn new_object(cx: &Context, result: Result<ObjectId, ScriptError>) -> *mut root::JSObject {
    match engine_result(cx, result) {
        Some(id) => object_ptr(cx, Some(id)),
        None => ptr::null_mut(),
    }
}

pub unsafe fn NewPromiseObject(
    cx: *mut root::JSContext,
    executor: JS::HandleObject,
) -> *mut root::JSObject {
    let cx = unsafe { context(cx) };
    let executor = unsafe { handle_object_or_null(executor) }.map(JsValue::object);
    let created = cx.heap.with_engine(|e| e.new_promise(executor.as_ref()));
    new_object(cx, created)
}

/// An unwrapped promise only: the facade's wrappers (WindowProxy, DOM proxies)
/// never wrap one.
pub unsafe fn IsPromiseObject(obj: JS::HandleObject) -> bool {
    // SAFETY: a Handle points to a rooted location.
    if unsafe { *obj.ptr }.is_null() {
        return false;
    }
    let (cx, id) = unsafe { promise_of(obj) };
    cx.heap.with_engine(|e| e.is_promise(id))
}

pub unsafe fn GetPromiseConstructor(cx: *mut root::JSContext) -> *mut root::JSObject {
    let cx = unsafe { context(cx) };
    let id = cx.heap.with_engine(|e| e.promise_constructor());
    object_ptr(cx, id)
}

pub unsafe fn GetPromisePrototype(cx: *mut root::JSContext) -> *mut root::JSObject {
    let cx = unsafe { context(cx) };
    let id = cx.heap.with_engine(|e| e.promise_prototype());
    object_ptr(cx, id)
}

fn status(cx: &Context, id: ObjectId) -> Option<(PromiseStatus, JsValue)> {
    cx.heap.with_engine(|e| e.promise_state(id).ok())
}

/// Pending for anything that is not a promise, as for a wrapper SpiderMonkey
/// cannot unwrap.
pub unsafe fn GetPromiseState(promise: JS::HandleObject) -> JS::PromiseState {
    let (cx, id) = unsafe { promise_of(promise) };
    match status(cx, id).map(|(s, _)| s) {
        Some(PromiseStatus::Fulfilled) => JS::PromiseState::Fulfilled,
        Some(PromiseStatus::Rejected) => JS::PromiseState::Rejected,
        _ => JS::PromiseState::Pending,
    }
}

/// [[PromiseResult]]; undefined while pending (mozjs glue `JS_GetPromiseResult`).
pub unsafe fn JS_GetPromiseResult(promise: JS::HandleObject, dest: JS::MutableHandleValue) {
    let (cx, id) = unsafe { promise_of(promise) };
    let value = cx.heap.with_engine(|e| {
        e.promise_state(id)
            .map_or(UndefinedValue(), |(_, v)| to_jsval(cx, e, &v))
    });
    // SAFETY: a MutableHandle points to a rooted location.
    unsafe { *dest.ptr = value };
}

pub unsafe fn GetPromiseIsHandled(promise: JS::HandleObject) -> bool {
    let (cx, id) = unsafe { promise_of(promise) };
    cx.heap
        .with_engine(|e| e.promise_is_handled(id))
        .unwrap_or(false)
}

fn mark_handled(cx: &Context, id: ObjectId) -> bool {
    let marked = cx.heap.with_engine(|e| e.set_promise_is_handled(id));
    engine_result(cx, marked).is_some()
}

pub unsafe fn SetSettledPromiseIsHandled(
    cx: *mut root::JSContext,
    promise: JS::HandleObject,
) -> bool {
    mark_handled(unsafe { context(cx) }, unsafe { handle_object(promise) })
}

pub unsafe fn SetAnyPromiseIsHandled(cx: *mut root::JSContext, promise: JS::HandleObject) -> bool {
    mark_handled(unsafe { context(cx) }, unsafe { handle_object(promise) })
}

pub unsafe fn CallOriginalPromiseResolve(
    cx: *mut root::JSContext,
    resolutionValue: JS::HandleValue,
) -> *mut root::JSObject {
    let cx = unsafe { context(cx) };
    // SAFETY: a Handle points to a rooted value.
    let value = unsafe { from_jsval(*resolutionValue.ptr) };
    let created = cx.heap.with_engine(|e| e.promise_resolve_original(&value));
    new_object(cx, created)
}

pub unsafe fn CallOriginalPromiseReject(
    cx: *mut root::JSContext,
    rejectionValue: JS::HandleValue,
) -> *mut root::JSObject {
    let cx = unsafe { context(cx) };
    // SAFETY: a Handle points to a rooted value.
    let value = unsafe { from_jsval(*rejectionValue.ptr) };
    let created = cx.heap.with_engine(|e| e.promise_reject_original(&value));
    new_object(cx, created)
}

unsafe fn settle(
    cx: *mut root::JSContext,
    promise: JS::HandleObject,
    value: JS::HandleValue,
    reject: bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let id = unsafe { handle_object(promise) };
    // SAFETY: a Handle points to a rooted value.
    let value = unsafe { from_jsval(*value.ptr) };
    let done = cx
        .heap
        .with_engine(|e| e.resolve_promise(id, &value, reject));
    engine_result(cx, done).is_some()
}

pub unsafe fn ResolvePromise(
    cx: *mut root::JSContext,
    promiseObj: JS::HandleObject,
    resolutionValue: JS::HandleValue,
) -> bool {
    unsafe { settle(cx, promiseObj, resolutionValue, false) }
}

pub unsafe fn RejectPromise(
    cx: *mut root::JSContext,
    promiseObj: JS::HandleObject,
    rejectionValue: JS::HandleValue,
) -> bool {
    unsafe { settle(cx, promiseObj, rejectionValue, true) }
}

unsafe fn then(
    cx: &Context,
    promise: JS::HandleObject,
    on_fulfilled: JS::HandleObject,
    on_rejected: JS::HandleObject,
    result: bool,
) -> Option<Option<ObjectId>> {
    let id = unsafe { handle_object(promise) };
    let fulfilled = unsafe { handle_object_or_null(on_fulfilled) }.map(JsValue::object);
    let rejected = unsafe { handle_object_or_null(on_rejected) }.map(JsValue::object);
    let done = cx.heap.with_engine(|e| {
        if !e.is_promise(id) {
            return Err(e.throw_new_error("TypeError", "value is not a Promise"));
        }
        e.perform_promise_then(id, fulfilled.as_ref(), rejected.as_ref(), result)
    });
    engine_result(cx, done)
}

pub unsafe fn CallOriginalPromiseThen(
    cx: *mut root::JSContext,
    promise: JS::HandleObject,
    onFulfilled: JS::HandleObject,
    onRejected: JS::HandleObject,
) -> *mut root::JSObject {
    let cx = unsafe { context(cx) };
    let derived = unsafe { then(cx, promise, onFulfilled, onRejected, true) };
    object_ptr(cx, derived.flatten())
}

pub unsafe fn AddPromiseReactions(
    cx: *mut root::JSContext,
    promise: JS::HandleObject,
    onFulfilled: JS::HandleObject,
    onRejected: JS::HandleObject,
) -> bool {
    let cx = unsafe { context(cx) };
    unsafe { then(cx, promise, onFulfilled, onRejected, false) }.is_some()
}

/// The engine makes no derived promise for these reactions, so a missing
/// `onRejected` never reports an unhandled rejection: both variants behave
/// the same (PROMISES.md "Embedder API").
pub unsafe fn AddPromiseReactionsIgnoringUnhandledRejection(
    cx: *mut root::JSContext,
    promise: JS::HandleObject,
    onFulfilled: JS::HandleObject,
    onRejected: JS::HandleObject,
) -> bool {
    unsafe { AddPromiseReactions(cx, promise, onFulfilled, onRejected) }
}

/// The user-input flag is facade state keyed by promise; the engine has no
/// such slot. New promises are DontCare, as in SpiderMonkey.
pub unsafe fn GetPromiseUserInputEventHandlingState(
    promise: *mut root::JSObject,
) -> JS::PromiseUserInputEventHandlingState {
    if promise.is_null() {
        return JS::PromiseUserInputEventHandlingState::DontCare;
    }
    let cell = unsafe { object_cell(promise) };
    cell.context()
        .heap
        .shared
        .user_input
        .borrow()
        .get(&cell.id)
        .copied()
        .unwrap_or(JS::PromiseUserInputEventHandlingState::DontCare)
}

pub unsafe fn SetPromiseUserInputEventHandlingState(
    promise: JS::HandleObject,
    state: JS::PromiseUserInputEventHandlingState,
) -> bool {
    let (cx, id) = unsafe { promise_of(promise) };
    if !cx.heap.with_engine(|e| e.is_promise(id)) {
        return false;
    }
    cx.heap.shared.user_input.borrow_mut().insert(id, state);
    true
}

// --- Functions with reserved slots (jsfriendapi.h) ---

/// Two reserved slots, facade-side like every other reserved slot (bit-exact).
const FUNCTION_RESERVED_SLOTS: usize = 2;

pub unsafe fn NewFunctionWithReserved(
    cx: *mut root::JSContext,
    call: root::JSNative,
    nargs: c_uint,
    flags: c_uint,
    name: *const c_char,
) -> *mut root::JSFunction {
    let fun = unsafe { super::natives::JS_NewFunction(cx, call, nargs, flags, name) };
    if fun.is_null() {
        return fun;
    }
    let cell = unsafe { object_cell(fun as *mut root::JSObject) };
    let cx = cell.context();
    // A traced host tag with no class (the cell keeps its null JSClass) makes
    // the collector ask `FacadeHost::trace` for the slots' object edges.
    let tagged = cx
        .heap
        .with_engine(|e| e.attach_host(cell.id, 0, 0, 0, true));
    if engine_result(cx, tagged).is_none() {
        return ptr::null_mut();
    }
    cx.heap
        .shared
        .slots
        .borrow_mut()
        .insert(cell.id, vec![UndefinedValue(); FUNCTION_RESERVED_SLOTS]);
    fun
}

pub unsafe fn FunctionHasNativeReserved(fun: *mut root::JSObject) -> bool {
    let cell = unsafe { object_cell(fun) };
    let shared = &cell.context().heap.shared;
    shared.native_of.borrow().contains_key(&cell.id) && shared.slots.borrow().contains_key(&cell.id)
}

/// A pointer into the slot vector: it never grows, so the address stays valid
/// while the function lives.
pub unsafe fn GetFunctionNativeReserved(
    fun: *mut root::JSObject,
    which: usize,
) -> *const JS::Value {
    let cell = unsafe { object_cell(fun) };
    cell.context()
        .heap
        .shared
        .slots
        .borrow()
        .get(&cell.id)
        .and_then(|slots| slots.get(which))
        .map(|slot| slot as *const JSVal)
        .expect("mozjs facade: function has no native reserved slot")
}

pub unsafe fn SetFunctionNativeReserved(
    fun: *mut root::JSObject,
    which: usize,
    val: *const JS::Value,
) {
    let cell = unsafe { object_cell(fun) };
    // SAFETY: `val` is the caller's value.
    let value = unsafe { *val };
    let shared = &cell.context().heap.shared;
    let mut slots = shared.slots.borrow_mut();
    let slot = slots
        .get_mut(&cell.id)
        .and_then(|slots| slots.get_mut(which))
        .expect("mozjs facade: function has no native reserved slot");
    *slot = value;
}

// --- HostPromiseRejectionTracker (§27.2.1.9) ---

/// Calls the embedder's `PromiseRejectionTrackerCallback` synchronously, as
/// SpiderMonkey does, with the engine parked so the callback may re-enter it.
pub(super) fn track_rejection(cx: &Context, engine: &mut Engine<'_>, id: ObjectId, handled: bool) {
    let Some((Some(callback), data)) = cx.state.borrow().promise_rejection_tracker else {
        return;
    };
    let cx_ptr = cx as *const Context as *mut root::JSContext;
    let promise = cx.heap.object(cx, engine, id, ptr::null());
    // SAFETY: the handle points to a local that outlives the call; the
    // object is a facade cell, which the collector treats as a root.
    let promise_h = unsafe { JS::HandleObject::from_marked_location(&promise) };
    let state = if handled {
        JS::PromiseRejectionHandlingState::Handled
    } else {
        JS::PromiseRejectionHandlingState::Unhandled
    };
    let ran = run_parked(cx, engine, || {
        // SAFETY: the embedder's callback, called with SpiderMonkey's contract.
        unsafe { callback(cx_ptr, false, promise_h, state, data) };
        true
    });
    if let Err(error) = ran {
        eprintln!("mozjs facade: promise rejection tracker skipped: {error:?}");
    }
}
