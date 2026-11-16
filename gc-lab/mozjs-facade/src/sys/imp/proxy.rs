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
//! `GetProxyHandlerExtra`) and its kind.
//!
//! `NewProxyObject` makes an engine host exotic object (HOST_EXOTIC.md, E3)
//! whose handler word is the handler pointer. [`exotic`] answers its
//! essential internal methods with the `ProxyTraps` of a DOM handler, as
//! jsglue.cpp's `ForwardingProxyHandler` over `js::BaseProxyHandler` does:
//! the mandatory traps always run, a missing optional trap (`has`, `get`,
//! `set`) is the ordinary algorithm (`ExoticReply::Default`), and a static
//! prototype never reaches the handler.
//!
//! Principals follow mozjs's jsglue.cpp `RustJSPrincipals`: a `JSPrincipals`
//! prefix (vtable, refcount) followed by the embedder's callbacks and private
//! pointer. `JS_DropPrincipals` calls the runtime's destroy callback when the
//! count reaches zero, as `JSRuntime::destroyPrincipals` does.

use std::cell::RefCell;
use std::os::raw::c_void;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use gc_lab::script::{Engine, ExoticOp, ExoticReply, ObjectId, ScriptError};

use super::natives::{from_jsval, key_of, run_parked, to_jsval};
use super::objects::{engine_result, object_cell, JS_GetReservedSlot, JS_SetReservedSlot, Shared};
use super::props::{complete_descriptor, id_of, partial_to_js, IdVectorCell};
use crate::sys::glue;
use crate::sys::imp::context;
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;
use crate::sys::jsval::JSVal;

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

// --- Proxy objects ---

/// What the facade keeps next to a proxy object.
#[derive(Clone, Copy)]
pub struct ProxyState {
    pub handler: *const c_void,
    /// `GetProxyPrivate`: the DOM expando object or a private pointer.
    pub private: JSVal,
    /// `TaggedProto::LazyProto`: [[GetPrototypeOf]] asks the handler.
    pub lazy_proto: bool,
}

/// `js::ProxyClass`: the class of a proxy made with a null `JSClass`.
static DEFAULT_PROXY_CLASS: root::JSClass = root::JSClass {
    name: c"Proxy".as_ptr(),
    flags: root::JSCLASS_IS_PROXY
        | ((2 & root::JSCLASS_RESERVED_SLOTS_MASK) << root::JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ptr::null(),
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

/// jsglue.cpp `NewProxyObject`: a proxy of `aClass` with a static prototype
/// `proto` (or a lazy one), its private value and the class's reserved slots.
pub unsafe fn NewProxyObject(
    aCx: *mut root::JSContext,
    aHandler: *const c_void,
    aPriv: JS::HandleValue,
    proto: *mut root::JSObject,
    aClass: *const root::JSClass,
    aLazyProto: bool,
) -> *mut root::JSObject {
    // A null class is `js::ProxyClass` (Proxy.cpp): "Proxy", two reserved
    // slots. Servo's DOM proxies (wrap.rs) store DOM_OBJECT_SLOT there.
    let aClass = if aClass.is_null() {
        &DEFAULT_PROXY_CLASS as *const root::JSClass
    } else {
        aClass
    };
    let cx = unsafe { context::context(aCx) };
    let heap = &cx.heap;
    let proto = (!aLazyProto && !proto.is_null()).then(|| unsafe { object_cell(proto) }.id);
    let created = heap.with_engine(|engine| {
        engine.create_host_exotic(proto, aHandler as u64, aClass as u64, 0, 0, true)
    });
    let Some(id) = engine_result(cx, created) else {
        return ptr::null_mut();
    };
    heap.init_slots(id, aClass);
    heap.shared.proxies.borrow_mut().insert(
        id,
        ProxyState {
            handler: aHandler,
            // SAFETY: a Handle points to a rooted, live value.
            private: unsafe { *aPriv.ptr },
            lazy_proto: aLazyProto,
        },
    );
    heap.with_engine(|engine| heap.object(cx, engine, id, aClass))
}

fn proxy_state(obj: *mut root::JSObject) -> Option<ProxyState> {
    // SAFETY: callers pass a JSObject* the facade handed out.
    let cell = unsafe { object_cell(obj) };
    cell.context()
        .heap
        .shared
        .proxies
        .borrow()
        .get(&cell.id)
        .copied()
}

fn expect_proxy(obj: *mut root::JSObject) -> ProxyState {
    proxy_state(obj).expect("mozjs facade: proxy entry point on a non-proxy object")
}

pub unsafe fn GetProxyPrivate(obj: *mut root::JSObject, dest: *mut JS::Value) {
    let private = expect_proxy(obj).private;
    // SAFETY: the caller's out-parameter.
    unsafe { *dest = private };
}

pub unsafe fn SetProxyPrivate(obj: *mut root::JSObject, expando: *const JS::Value) {
    // SAFETY: the caller's value; `obj` is a facade cell.
    let (value, cell) = unsafe { (*expando, object_cell(obj)) };
    let shared = &cell.context().heap.shared;
    shared
        .proxies
        .borrow_mut()
        .get_mut(&cell.id)
        .expect("mozjs facade: SetProxyPrivate on a non-proxy object")
        .private = value;
}

pub unsafe fn GetProxyReservedSlot(obj: *mut root::JSObject, slot: u32, dest: *mut JS::Value) {
    unsafe { JS_GetReservedSlot(obj, slot, dest) }
}

pub unsafe fn SetProxyReservedSlot(obj: *mut root::JSObject, slot: u32, val: *const JS::Value) {
    unsafe { JS_SetReservedSlot(obj, slot, val) }
}

pub unsafe fn GetProxyHandler(obj: *mut root::JSObject) -> *const c_void {
    expect_proxy(obj).handler
}

pub unsafe fn GetProxyHandlerExtra(obj: *mut root::JSObject) -> *const c_void {
    unsafe { handler(expect_proxy(obj).handler) }.extra
}

/// `js::IsProxy(obj) && handler->family() == &HandlerFamily`.
pub unsafe fn IsProxyHandlerFamily(obj: *mut root::JSObject) -> bool {
    proxy_state(obj).is_some_and(|p| unsafe { handler(p.handler) }.family == family())
}

// --- Trap dispatch (FacadeHost::exotic) ---

fn op_ok(result: &JS::ObjectOpResult) -> bool {
    result.code_ == JS::ObjectOpResult_SpecialCodes::OkCode as usize
}

fn blank_result() -> JS::ObjectOpResult {
    JS::ObjectOpResult {
        code_: JS::ObjectOpResult_SpecialCodes::Uninitialized as usize,
    }
}

/// Answer `op` on the proxy `object` with its handler's traps. Each trap
/// runs with the engine parked (natives.rs `run_parked`), so it can reenter
/// through JSAPI; a trap returning false throws the pending exception.
pub(super) fn exotic(
    shared: &Rc<Shared>,
    engine: &mut Engine<'_>,
    object: ObjectId,
    handler_word: u64,
    op: ExoticOp,
) -> Result<ExoticReply, ScriptError> {
    let cx_ptr = shared.context.get() as *mut root::JSContext;
    // SAFETY: set by JS_NewContext; the context outlives its engine.
    let cx = unsafe { context::context(cx_ptr) };
    // SAFETY: handler words are handler pointers made by this module.
    let Some(traps) = (unsafe { handler(handler_word as usize as *const c_void) }).traps else {
        // Wrapper singletons carry no traps: the ordinary algorithms (D).
        return Ok(ExoticReply::Default);
    };
    let lazy = shared
        .proxies
        .borrow()
        .get(&object)
        .is_some_and(|p| p.lazy_proto);
    let proxy = cx.heap.object(cx, engine, object, ptr::null());
    // SAFETY (all handles below): they point to locals that outlive the trap
    // call; object values are facade cells, which the collector treats as roots.
    let proxy_h = unsafe { JS::HandleObject::from_marked_location(&proxy) };
    let mut result = blank_result();
    match op {
        ExoticOp::GetPrototypeOf => {
            let (true, Some(trap)) = (lazy, traps.getPrototype) else {
                return Ok(ExoticReply::Default);
            };
            let mut proto: *mut root::JSObject = ptr::null_mut();
            let proto_h = unsafe { JS::MutableHandleObject::from_marked_location(&mut proto) };
            run_parked(cx, engine, || unsafe { trap(cx_ptr, proxy_h, proto_h) })?;
            Ok(ExoticReply::Prototype(
                (!proto.is_null()).then(|| unsafe { object_cell(proto) }.id),
            ))
        }
        ExoticOp::SetPrototypeOf(proto) => {
            let (true, Some(trap)) = (lazy, traps.setPrototype) else {
                return Ok(ExoticReply::Default);
            };
            let proto = proto.map_or(ptr::null_mut(), |p| {
                cx.heap.object(cx, engine, p, ptr::null())
            });
            let proto_h = unsafe { JS::HandleObject::from_marked_location(&proto) };
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, proto_h, &mut result)
            })?;
            Ok(ExoticReply::Bool(op_ok(&result)))
        }
        ExoticOp::IsExtensible => {
            let Some(trap) = traps.isExtensible else {
                return Ok(ExoticReply::Default);
            };
            let mut extensible = false;
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, &mut extensible)
            })?;
            Ok(ExoticReply::Bool(extensible))
        }
        ExoticOp::PreventExtensions => {
            let Some(trap) = traps.preventExtensions else {
                return Ok(ExoticReply::Default);
            };
            run_parked(cx, engine, || unsafe { trap(cx_ptr, proxy_h, &mut result) })?;
            Ok(ExoticReply::Bool(op_ok(&result)))
        }
        ExoticOp::GetOwnProperty(key) => {
            let Some(trap) = traps.getOwnPropertyDescriptor else {
                return Ok(ExoticReply::Default);
            };
            let id = id_of(cx, &key);
            let id_h = unsafe { JS::HandleId::from_marked_location(&id) };
            let mut desc = partial_to_js(cx, engine, &Default::default());
            let desc_h = unsafe { JS::MutableHandle::from_marked_location(&mut desc) };
            let mut is_none = true;
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, id_h, desc_h, &mut is_none)
            })?;
            Ok(ExoticReply::Property(
                (!is_none).then(|| unsafe { complete_descriptor(&desc) }),
            ))
        }
        ExoticOp::DefineOwnProperty(key, partial) => {
            let Some(trap) = traps.defineProperty else {
                return Ok(ExoticReply::Default);
            };
            let id = id_of(cx, &key);
            let id_h = unsafe { JS::HandleId::from_marked_location(&id) };
            let desc = partial_to_js(cx, engine, &partial);
            let desc_h = unsafe { JS::Handle::from_marked_location(&desc) };
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, id_h, desc_h, &mut result)
            })?;
            Ok(ExoticReply::Bool(op_ok(&result)))
        }
        ExoticOp::HasProperty(key) => {
            let Some(trap) = traps.has else {
                return Ok(ExoticReply::Default);
            };
            let id = id_of(cx, &key);
            let id_h = unsafe { JS::HandleId::from_marked_location(&id) };
            let mut found = false;
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, id_h, &mut found)
            })?;
            Ok(ExoticReply::Bool(found))
        }
        ExoticOp::Get(key, receiver) => {
            let Some(trap) = traps.get else {
                return Ok(ExoticReply::Default);
            };
            let id = id_of(cx, &key);
            let id_h = unsafe { JS::HandleId::from_marked_location(&id) };
            let receiver = to_jsval(cx, engine, &receiver);
            let receiver_h = unsafe { JS::HandleValue::from_marked_location(&receiver) };
            let mut vp = JSVal::default();
            let vp_h = unsafe { JS::MutableHandleValue::from_marked_location(&mut vp) };
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, receiver_h, id_h, vp_h)
            })?;
            // SAFETY: the trap's result value.
            Ok(ExoticReply::Value(unsafe { from_jsval(vp) }))
        }
        ExoticOp::Set(key, value, receiver) => {
            let Some(trap) = traps.set else {
                return Ok(ExoticReply::Default);
            };
            let id = id_of(cx, &key);
            let id_h = unsafe { JS::HandleId::from_marked_location(&id) };
            let value = to_jsval(cx, engine, &value);
            let value_h = unsafe { JS::HandleValue::from_marked_location(&value) };
            let receiver = to_jsval(cx, engine, &receiver);
            let receiver_h = unsafe { JS::HandleValue::from_marked_location(&receiver) };
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, id_h, value_h, receiver_h, &mut result)
            })?;
            Ok(ExoticReply::Bool(op_ok(&result)))
        }
        ExoticOp::Delete(key) => {
            let Some(trap) = traps.delete_ else {
                return Ok(ExoticReply::Default);
            };
            let id = id_of(cx, &key);
            let id_h = unsafe { JS::HandleId::from_marked_location(&id) };
            run_parked(cx, engine, || unsafe {
                trap(cx_ptr, proxy_h, id_h, &mut result)
            })?;
            Ok(ExoticReply::Bool(op_ok(&result)))
        }
        ExoticOp::OwnPropertyKeys => {
            let Some(trap) = traps.ownPropertyKeys else {
                return Ok(ExoticReply::Default);
            };
            let ids = IdVectorCell(RefCell::new(Vec::new()));
            let ids_h = JS::MutableHandleIdVector {
                ptr: &ids as *const IdVectorCell as *mut _,
            };
            run_parked(cx, engine, || unsafe { trap(cx_ptr, proxy_h, ids_h) })?;
            // SAFETY: the trap appended valid ids.
            let keys = ids.0.take().into_iter().map(|id| unsafe { key_of(id) });
            Ok(ExoticReply::Keys(keys.collect()))
        }
    }
}
