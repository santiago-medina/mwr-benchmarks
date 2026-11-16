/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Stack capture and SavedFrame accessors (js/public/Stack.h,
//! js/src/vm/SavedStacks.cpp), as Servo's console reaches them through
//! `CapturedJSStack` (components/script/dom/console.rs `get_js_stack`).
//!
//! The engine keeps no frame stack with source positions yet, so a capture
//! records no frames: `CaptureCurrentStack` succeeds with a null stack, which
//! is SpiderMonkey's answer when there are no frames to capture. The
//! accessors follow SpiderMonkey's contract for a null (or non-SavedFrame)
//! object: they write the default (empty source, line and column 0, null
//! name and parent) and return `AccessDenied`. `BuildStackString` of a null
//! stack is the empty string. MOZJS_FACADE.md "Stack capture" lists the gap.

use super::context::context;
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;

/// The `mozilla::Variant` tag the facade writes: 0 AllFrames, 1 MaxFrames,
/// 2 FirstSubsumedFrame; word 1 holds the frame limit. Only the facade
/// reads a `StackCapture`, so the layout is private to it.
fn write_capture(capture: *mut JS::StackCapture, tag: u64, max: u64) {
    // SAFETY: `capture` points at caller storage of `StackCapture`'s size
    // (`CapturedJSStack::new` passes a `MaybeUninit`).
    unsafe { capture.write(root::__BindgenOpaqueArray([tag, max, 0, 0])) };
}

pub unsafe fn JS_StackCapture_AllFrames(capture: *mut JS::StackCapture) {
    write_capture(capture, 0, 0);
}

pub unsafe fn JS_StackCapture_MaxFrames(max: u32, capture: *mut JS::StackCapture) {
    write_capture(capture, 1, u64::from(max));
}

pub unsafe fn JS_StackCapture_FirstSubsumedFrame(
    _cx: *mut root::JSContext,
    _ignore_self_hosted_frames: bool,
    capture: *mut JS::StackCapture,
) {
    write_capture(capture, 2, 0);
}

/// `JS::CaptureCurrentStack`: no frames are recorded yet (module docs).
pub unsafe fn CaptureCurrentStack(
    _cx: *mut root::JSContext,
    stackp: JS::MutableHandleObject,
    _capture: *mut JS::StackCapture,
    _start_after: JS::HandleObject,
) -> bool {
    unsafe { *stackp.ptr = std::ptr::null_mut() };
    true
}

/// `JS::BuildStackString`: a null stack formats as the empty string.
pub unsafe fn BuildStackString(
    cx: *mut root::JSContext,
    _principals: *mut root::JSPrincipals,
    _stack: JS::HandleObject,
    stringp: JS::MutableHandleString,
    _indent: usize,
    _format: root::js::StackFormat,
) -> bool {
    unsafe { *stringp.ptr = context(cx).heap.atoms.string(String::new()) };
    true
}

pub unsafe fn GetSavedFrameSource(
    cx: *mut root::JSContext,
    _principals: *mut root::JSPrincipals,
    _frame: JS::Handle<*mut root::JSObject>,
    sourcep: JS::MutableHandle<*mut root::JSString>,
    _self_hosted: JS::SavedFrameSelfHosted,
) -> JS::SavedFrameResult {
    unsafe { *sourcep.ptr = context(cx).heap.atoms.string(String::new()) };
    JS::SavedFrameResult::AccessDenied
}

pub unsafe fn GetSavedFrameLine(
    _cx: *mut root::JSContext,
    _principals: *mut root::JSPrincipals,
    _frame: JS::Handle<*mut root::JSObject>,
    linep: *mut u32,
    _self_hosted: JS::SavedFrameSelfHosted,
) -> JS::SavedFrameResult {
    unsafe { *linep = 0 };
    JS::SavedFrameResult::AccessDenied
}

pub unsafe fn GetSavedFrameColumn(
    _cx: *mut root::JSContext,
    _principals: *mut root::JSPrincipals,
    _frame: JS::Handle<*mut root::JSObject>,
    columnp: *mut JS::TaggedColumnNumberOneOrigin,
    _self_hosted: JS::SavedFrameSelfHosted,
) -> JS::SavedFrameResult {
    unsafe { (*columnp).value_ = 0 };
    JS::SavedFrameResult::AccessDenied
}

pub unsafe fn GetSavedFrameFunctionDisplayName(
    _cx: *mut root::JSContext,
    _principals: *mut root::JSPrincipals,
    _frame: JS::Handle<*mut root::JSObject>,
    namep: JS::MutableHandle<*mut root::JSString>,
    _self_hosted: JS::SavedFrameSelfHosted,
) -> JS::SavedFrameResult {
    unsafe { *namep.ptr = std::ptr::null_mut() };
    JS::SavedFrameResult::AccessDenied
}

pub unsafe fn GetSavedFrameParent(
    _cx: *mut root::JSContext,
    _principals: *mut root::JSPrincipals,
    _frame: JS::Handle<*mut root::JSObject>,
    parentp: JS::MutableHandle<*mut root::JSObject>,
    _self_hosted: JS::SavedFrameSelfHosted,
) -> JS::SavedFrameResult {
    unsafe { *parentp.ptr = std::ptr::null_mut() };
    JS::SavedFrameResult::AccessDenied
}
