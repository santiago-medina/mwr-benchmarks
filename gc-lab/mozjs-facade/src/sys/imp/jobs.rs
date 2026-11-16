/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! The microtask queue (js/public/Promise.h, MOZJS_FACADE.md "Microtasks").
//!
//! SpiderMonkey hands the queue to the embedder: a `GenericMicroTask` is a
//! `JS::Value`, either an object (a `JSMicroTask*`, an engine promise job) or
//! anything else the embedder enqueued itself (Servo: a `PrivateValue` boxing a
//! Rust runnable). The facade owns that queue in `Shared::microtasks`; the
//! engine reports each promise job through `Host::enqueue_job`
//! (HostEnqueuePromiseJob, ECMA-262 §9.5.5) and runs it when Servo calls
//! `RunJSMicroTask` (`Engine::run_host_job`), so the order is the queue's.
//!
//! A `JSMicroTask*` is the cell of the engine object holding the job record.
//! It is made at dequeue time and stops being a root once the job ran.

use std::ptr;

use super::context::context;
use super::objects::{engine_result, object_cell, Heap};
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;
use crate::sys::jsval::{JSVal, ObjectValue, UndefinedValue};

pub(super) enum Microtask {
    /// A value the embedder enqueued, returned as is.
    Generic(JSVal),
    /// An engine promise job, by serial.
    Js(u64),
}

/// Drop engine jobs that no longer exist (cancelled after an engine fault),
/// so a dequeue never yields a job that cannot run.
fn prune(heap: &Heap) {
    let has_js = heap
        .shared
        .microtasks
        .borrow()
        .iter()
        .any(|t| matches!(t, Microtask::Js(_)));
    if !has_js || heap.engine.borrow().is_none() {
        return;
    }
    heap.with_engine(|engine| {
        heap.shared.microtasks.borrow_mut().retain(|t| match t {
            Microtask::Js(serial) => engine.job_object(*serial).is_some(),
            Microtask::Generic(_) => true,
        })
    });
}

fn task_value(cx: *mut root::JSContext, task: Microtask) -> JSVal {
    match task {
        Microtask::Generic(value) => value,
        Microtask::Js(serial) => {
            let context = unsafe { context(cx) };
            let heap = &context.heap;
            let object = heap.with_engine(|engine| {
                let id = engine.job_object(serial)?;
                Some((id, heap.object(context, engine, id, ptr::null())))
            });
            match object {
                Some((id, object)) => {
                    heap.shared.job_tasks.borrow_mut().insert(id, serial);
                    ObjectValue(object)
                }
                // prune() ran first; a job cancelled since is skipped.
                None => UndefinedValue(),
            }
        }
    }
}

fn dequeue(cx: *mut root::JSContext) -> JSVal {
    let heap = &unsafe { context(cx) }.heap;
    prune(heap);
    let next = heap.shared.microtasks.borrow_mut().pop_front();
    next.map_or(UndefinedValue(), |task| task_value(cx, task))
}

pub unsafe fn EnqueueMicroTask(
    cx: *mut root::JSContext,
    entry: *const JS::GenericMicroTask,
) -> bool {
    let value = unsafe { *entry };
    let heap = &unsafe { context(cx) }.heap;
    heap.shared
        .microtasks
        .borrow_mut()
        .push_back(Microtask::Generic(value));
    true
}

pub unsafe fn EnqueueDebugMicroTask(
    cx: *mut root::JSContext,
    entry: *const JS::GenericMicroTask,
) -> bool {
    unsafe { EnqueueMicroTask(cx, entry) }
}

pub unsafe fn PrependMicroTask(
    cx: *mut root::JSContext,
    entry: *const JS::GenericMicroTask,
) -> bool {
    let value = unsafe { *entry };
    let heap = &unsafe { context(cx) }.heap;
    heap.shared
        .microtasks
        .borrow_mut()
        .push_front(Microtask::Generic(value));
    true
}

pub unsafe fn HasAnyMicroTasks(cx: *mut root::JSContext) -> bool {
    let heap = &unsafe { context(cx) }.heap;
    prune(heap);
    !heap.shared.microtasks.borrow().is_empty()
}

pub unsafe fn HasRegularMicroTasks(cx: *mut root::JSContext) -> bool {
    unsafe { HasAnyMicroTasks(cx) }
}

pub unsafe fn HasDebuggerMicroTasks(_cx: *mut root::JSContext) -> bool {
    false
}

pub unsafe fn GetRegularMicroTaskCount(cx: *mut root::JSContext) -> usize {
    let heap = &unsafe { context(cx) }.heap;
    prune(heap);
    heap.shared.microtasks.borrow().len()
}

pub unsafe fn JS_DequeueNextMicroTask(
    cx: *mut root::JSContext,
    task: JS::MutableHandle<JS::GenericMicroTask>,
) {
    let value = dequeue(cx);
    unsafe { *task.ptr = value };
}

pub unsafe fn DequeueNextRegularMicroTask(cx: *mut root::JSContext) -> JS::GenericMicroTask {
    dequeue(cx)
}

pub unsafe fn DequeueNextDebuggerMicroTask(_cx: *mut root::JSContext) -> JS::GenericMicroTask {
    UndefinedValue()
}

pub unsafe fn PeekNextMicroTask(cx: *mut root::JSContext) -> JS::GenericMicroTask {
    let heap = &unsafe { context(cx) }.heap;
    prune(heap);
    let front = match heap.shared.microtasks.borrow().front() {
        None => return UndefinedValue(),
        Some(Microtask::Generic(value)) => return *value,
        Some(Microtask::Js(serial)) => *serial,
    };
    task_value(cx, Microtask::Js(front))
}

pub unsafe fn IsJSMicroTask(hv: *const JS::GenericMicroTask) -> bool {
    unsafe { (*hv).is_object() }
}

pub unsafe fn ToUnwrappedJSMicroTask(
    generic_micro_task: *const JS::GenericMicroTask,
) -> *mut JS::JSMicroTask {
    let value = unsafe { *generic_micro_task };
    if value.is_object() {
        value.to_object()
    } else {
        ptr::null_mut()
    }
}

pub unsafe fn ToMaybeWrappedJSMicroTask(
    generic_micro_task: *const JS::GenericMicroTask,
) -> *mut JS::JSMicroTask {
    unsafe { ToUnwrappedJSMicroTask(generic_micro_task) }
}

/// The global of the job's realm: the handler's [[Realm]] (NewPromiseReactionJob
/// step 3), else the realm the job was queued in. Never null for a pending
/// job: Servo skips a task with a null global and would never run it.
pub unsafe fn GetExecutionGlobalFromJSMicroTask(
    entry: *mut JS::JSMicroTask,
) -> *mut root::JSObject {
    let cell = unsafe { object_cell(entry) };
    let heap = &cell.context().heap;
    let serial = heap.shared.job_tasks.borrow().get(&cell.id).copied();
    let realms = heap.with_engine(|engine| {
        let handler = serial.and_then(|s| engine.job_realm(s));
        [
            handler,
            Some(engine.object_realm(cell.id)),
            Some(engine.current_realm_id()),
        ]
    });
    realms
        .into_iter()
        .flatten()
        .map(|realm| heap.realm_global_ptr(realm))
        .find(|global| !global.is_null())
        .unwrap_or(ptr::null_mut())
}

/// No incumbent tracking yet (HostMakeJobCallback's [[HostDefined]]): both
/// outputs null, which Servo reads as "run without an incumbent".
pub unsafe fn MaybeGetHostDefinedDataFromJSMicroTask(
    _entry: *mut JS::JSMicroTask,
    incumbent_global: JS::MutableHandleObject,
    optional_host_defined_data: JS::MutableHandleObject,
) -> bool {
    unsafe {
        *incumbent_global.ptr = ptr::null_mut();
        *optional_host_defined_data.ptr = ptr::null_mut();
    }
    true
}

pub unsafe fn MaybeGetAllocationSiteFromJSMicroTask(
    _entry: *mut JS::JSMicroTask,
    out: JS::MutableHandleObject,
) -> bool {
    unsafe { *out.ptr = ptr::null_mut() };
    true
}

pub unsafe fn MaybeGetPromiseFromJSMicroTask(_entry: *mut JS::JSMicroTask) -> *mut root::JSObject {
    ptr::null_mut()
}

pub unsafe fn GetFlowIdFromJSMicroTask(_entry: *mut JS::JSMicroTask, _uid: *mut u64) -> bool {
    false
}

pub unsafe fn RunJSMicroTask(
    cx: *mut root::JSContext,
    entry: JS::Handle<*mut JS::JSMicroTask>,
) -> bool {
    let context = unsafe { context(cx) };
    let object = unsafe { *entry.ptr };
    if object.is_null() {
        return false;
    }
    let id = unsafe { object_cell(object) }.id;
    let Some(serial) = context.heap.shared.job_tasks.borrow_mut().remove(&id) else {
        return false;
    };
    let result = context
        .heap
        .with_engine(|engine| engine.run_host_job(serial));
    context.heap.forget_cell(id);
    engine_result(context, result).is_some()
}

pub unsafe fn GetJobQueue(cx: *mut root::JSContext) -> *mut JS::JobQueue {
    unsafe { context(cx) }
        .state
        .borrow()
        .job_queue
        .unwrap_or(ptr::null_mut())
}

/// Hints for skipping await jobs on an empty queue; the engine always queues.
pub unsafe fn JobQueueIsEmpty(_cx: *mut root::JSContext) {}

pub unsafe fn JobQueueMayNotBeEmpty(_cx: *mut root::JSContext) {}
