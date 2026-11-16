//! JIT-info glue: Servo's WebIDL bindings route every DOM attribute getter,
//! setter and regular method through `CallJitGetterOp`, `CallJitSetterOp` and
//! `CallJitMethodOp` (mozjs `jsglue.cpp`). Each one unpacks the JSNative
//! `vp` array into the argument struct the `JSJitInfo` op expects and calls
//! it; no engine state is touched.
//!
//! `vp` layout (JS::CallArgs): `vp[0]` callee / return value, `vp[1]` this
//! (magic `JS_IS_CONSTRUCTING` when constructing), `vp[2..2 + argc]` args.
#![allow(non_snake_case)]

use std::marker::PhantomData;
use std::os::raw::{c_uint, c_void};

use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;

fn value_handle(ptr: *mut JS::Value) -> JS::MutableHandle<JS::Value> {
    JS::MutableHandle {
        _phantom_0: PhantomData,
        ptr,
    }
}

/// `info->getter(cx, thisObj, specializedThis, JSJitGetterCallArgs(args))`:
/// the getter writes its result through `args.rval()`, which is `vp[0]`.
pub unsafe fn CallJitGetterOp(
    info: *const root::JSJitInfo,
    cx: *mut root::JSContext,
    thisObj: JS::HandleObject,
    specializedThis: *mut c_void,
    _argc: c_uint,
    vp: *mut JS::Value,
) -> bool {
    let Some(getter) = (unsafe { (*info).__bindgen_anon_1.getter }) else {
        return false;
    };
    let args = root::JSJitGetterCallArgs {
        _base: value_handle(vp),
    };
    unsafe { getter(cx, thisObj, specializedThis, args) }
}

/// `info->setter(cx, thisObj, specializedThis, JSJitSetterCallArgs(args))`:
/// the setter reads `args[0]`, which is `vp[2]`. The caller sets the
/// return value itself.
pub unsafe fn CallJitSetterOp(
    info: *const root::JSJitInfo,
    cx: *mut root::JSContext,
    thisObj: JS::HandleObject,
    specializedThis: *mut c_void,
    _argc: c_uint,
    vp: *mut JS::Value,
) -> bool {
    let Some(setter) = (unsafe { (*info).__bindgen_anon_1.setter }) else {
        return false;
    };
    let args = root::JSJitSetterCallArgs {
        _base: value_handle(unsafe { vp.add(2) }),
    };
    unsafe { setter(cx, thisObj, specializedThis, args) }
}

/// `info->method(cx, thisObj, specializedThis, JSJitMethodCallArgs(args))`:
/// the method sees `argv = vp + 2`, `argc` and the constructing bit, and its
/// `rval()` is `argv[-2]`, that is `vp[0]`.
pub unsafe fn CallJitMethodOp(
    info: *const root::JSJitInfo,
    cx: *mut root::JSContext,
    thisObj: JS::HandleObject,
    specializedThis: *mut c_void,
    argc: u32,
    vp: *mut JS::Value,
) -> bool {
    let Some(method) = (unsafe { (*info).__bindgen_anon_1.method }) else {
        return false;
    };
    let constructing = unsafe { (*vp.add(1)).is_magic() };
    let args = root::JSJitMethodCallArgs {
        argv_: unsafe { vp.add(2) },
        argc_: argc,
        _bitfield_align_1: [],
        _bitfield_1: root::JSJitMethodCallArgs::new_bitfield_1(constructing, false),
        __bindgen_padding_0: [0; 3],
    };
    unsafe { method(cx, thisObj, specializedThis, &args) }
}
