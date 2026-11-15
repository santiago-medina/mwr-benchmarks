/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Proxy handlers and principals: the calls Servo's `script::init`
//! (components/script/engine/init.rs) makes before it creates any runtime.
//! `proxyhandler::init` asks for the DOM proxy handler family, and the
//! generated `RegisterProxyHandlers` creates one handler per proxy
//! interface from a `ProxyTraps` table.
//!
//! A handler pointer points to a leaked [`Handler`]. It keeps a copy of the
//! traps, the `extra` pointer (the interface's `DOMClass`, read back by
//! `GetProxyHandlerExtra`) and its kind. Proxy objects themselves come with
//! the object cluster; until then the handlers are only created and stored.
//!
//! Principals follow mozjs's jsglue.cpp `RustJSPrincipals`: a `JSPrincipals`
//! prefix (vtable, refcount) followed by the embedder's callbacks and private
//! pointer. `JS_DropPrincipals` calls the runtime's destroy callback when the
//! count reaches zero, as `JSRuntime::destroyPrincipals` does.

use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use crate::sys::glue;
use crate::sys::imp::context;
use crate::sys::jsapi as root;

/// What a handler pointer refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandlerKind {
    /// `CreateProxyHandler`: a DOM proxy (`ForwardingProxyHandler` in jsglue).
    Dom,
    /// `CreateWrapperProxyHandler`: a wrapper with embedder traps.
    Wrapper,
    /// `GetCrossCompartmentWrapper`: `js::CrossCompartmentWrapper::singleton`.
    CrossCompartment,
    /// `GetSecurityWrapper`: the opaque cross-origin wrapper singleton.
    Security,
}

/// The object behind a proxy handler pointer.
#[repr(C)]
pub struct Handler {
    pub family: *const c_void,
    pub kind: HandlerKind,
    pub traps: Option<glue::ProxyTraps>,
    pub extra: *const c_void,
}

/// The DOM proxy handler family. Only its address matters: SpiderMonkey
/// compares `handler->family()` against it to recognise DOM proxies.
static FAMILY: u8 = 0;

fn family() -> *const c_void {
    &FAMILY as *const u8 as *const c_void
}

fn leak(kind: HandlerKind, traps: Option<glue::ProxyTraps>, extra: *const c_void) -> usize {
    Box::into_raw(Box::new(Handler {
        family: family(),
        kind,
        traps,
        extra,
    })) as usize
}

/// The handler a facade handler pointer refers to.
///
/// # Safety
/// `handler` must come from this module.
pub unsafe fn handler<'a>(handler: *const c_void) -> &'a Handler {
    unsafe { &*(handler as *const Handler) }
}

pub unsafe fn GetProxyHandlerFamily() -> *const c_void {
    family()
}

pub unsafe fn CreateProxyHandler(
    aTraps: *const glue::ProxyTraps,
    aExtra: *const c_void,
) -> *const c_void {
    let traps = unsafe { ptr::read(aTraps) };
    leak(HandlerKind::Dom, Some(traps), aExtra) as *const c_void
}

pub unsafe fn CreateWrapperProxyHandler(aTraps: *const glue::ProxyTraps) -> *const c_void {
    let traps = unsafe { ptr::read(aTraps) };
    leak(HandlerKind::Wrapper, Some(traps), ptr::null()) as *const c_void
}

pub unsafe fn DeleteWrapperProxyHandler(handler: *const c_void) {
    if !handler.is_null() {
        drop(unsafe { Box::from_raw(handler as *mut Handler) });
    }
}

pub unsafe fn GetCrossCompartmentWrapper() -> *const c_void {
    static CCW: OnceLock<usize> = OnceLock::new();
    *CCW.get_or_init(|| leak(HandlerKind::CrossCompartment, None, ptr::null())) as *const c_void
}

pub unsafe fn GetSecurityWrapper() -> *const c_void {
    static SECURITY: OnceLock<usize> = OnceLock::new();
    *SECURITY.get_or_init(|| leak(HandlerKind::Security, None, ptr::null())) as *const c_void
}

/// mozjs's `RustJSPrincipals`.
#[repr(C)]
struct RustPrincipals {
    base: root::JSPrincipals,
    callbacks: *const glue::JSPrincipalsCallbacks,
    private: *mut c_void,
}

pub unsafe fn CreateRustJSPrincipals(
    callbacks: *const glue::JSPrincipalsCallbacks,
    privateData: *mut c_void,
) -> *mut root::JSPrincipals {
    Box::into_raw(Box::new(RustPrincipals {
        base: root::JSPrincipals {
            vtable_: ptr::null(),
            refcount: 0,
        },
        callbacks,
        private: privateData,
    })) as *mut root::JSPrincipals
}

pub unsafe fn DestroyRustJSPrincipals(principals: *mut root::JSPrincipals) {
    if !principals.is_null() {
        drop(unsafe { Box::from_raw(principals as *mut RustPrincipals) });
    }
}

pub unsafe fn GetRustJSPrincipalsPrivate(principals: *mut root::JSPrincipals) -> *mut c_void {
    unsafe { (*(principals as *mut RustPrincipals)).private }
}

fn refcount<'a>(principals: *mut root::JSPrincipals) -> &'a AtomicU32 {
    // SAFETY: callers pass a live principals object; SpiderMonkey's refcount
    // is atomic too (`mozilla::Atomic<int32_t>`).
    unsafe { AtomicU32::from_ptr(ptr::addr_of_mut!((*principals).refcount)) }
}

pub unsafe fn JS_HoldPrincipals(principals: *mut root::JSPrincipals) {
    refcount(principals).fetch_add(1, Ordering::AcqRel);
}

pub unsafe fn JS_DropPrincipals(cx: *mut root::JSContext, principals: *mut root::JSPrincipals) {
    if refcount(principals).fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    let destroy = unsafe { context::context(cx) }
        .state
        .borrow()
        .destroy_principals;
    match destroy {
        Some(destroy) => unsafe { destroy(principals) },
        None => unsafe { DestroyRustJSPrincipals(principals) },
    }
}
