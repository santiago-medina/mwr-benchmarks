//! The context cluster (`src/sys/imp/context.rs`): the calls `JSEngine::init`
//! and `Runtime::new` make, plus stack rooting over the `RootingContext`
//! prefix of the facade's context. `JSEngine::init` runs once per process, so
//! the runtime part lives in one test. Also the proxy handler and
//! principals calls `script::init` makes (`src/sys/imp/proxy.rs`).
use mozjs::glue::{
    CreateProxyHandler, CreateRustJSPrincipals, CreateWrapperProxyHandler,
    DeleteWrapperProxyHandler, DestroyRustJSPrincipals, GetCrossCompartmentWrapper,
    GetProxyHandlerFamily, GetRustJSPrincipalsPrivate, GetSecurityWrapper, ProxyTraps,
};
use mozjs::jsapi::{JSGCParamKey, JS_GetGCParameter, JS_GetRuntime, JS_SetGCParameter};
use mozjs::jsapi::{
    JSPrincipals, JS_DropPrincipals, JS_HoldPrincipals, JS_InitDestroyPrincipalsCallback,
};
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::{JSEngine, Runtime};
use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn engine_runtime_and_rooting() {
    let engine = JSEngine::init().expect("JS_Init");
    let mut runtime = Runtime::new(engine.handle());
    let raw = unsafe { runtime.cx_no_gc().raw_cx_no_gc() };
    unsafe {
        assert_eq!(JS_GetRuntime(raw) as usize, raw as usize);
        JS_SetGCParameter(raw, JSGCParamKey::JSGC_MAX_BYTES, 1234);
        assert_eq!(JS_GetGCParameter(raw, JSGCParamKey::JSGC_MAX_BYTES), 1234);
    }
    {
        let cx = runtime.cx();
        rooted!(&in(cx) let mut v = UndefinedValue());
        assert!(v.get().is_undefined());
        v.set(Int32Value(7));
        let inner: JSVal = v.get();
        assert_eq!(inner.to_int32(), 7);
    }
    unsafe {
        // Without a destroy callback the last drop frees the principals.
        let principals = CreateRustJSPrincipals(ptr::null(), 0x1234 as *mut c_void);
        assert_eq!(GetRustJSPrincipalsPrivate(principals) as usize, 0x1234);
        JS_HoldPrincipals(principals);
        JS_HoldPrincipals(principals);
        JS_DropPrincipals(raw, principals);
        assert_eq!((*principals).refcount, 1);
        // With one, the runtime's callback runs instead.
        JS_InitDestroyPrincipalsCallback(raw, Some(destroy));
        JS_DropPrincipals(raw, principals);
        assert_eq!(DESTROYED.load(Ordering::SeqCst), 1);
    }
    drop(runtime);
    drop(engine);
}

static DESTROYED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn destroy(principals: *mut JSPrincipals) {
    DESTROYED.fetch_add(1, Ordering::SeqCst);
    unsafe { DestroyRustJSPrincipals(principals) };
}

#[test]
fn proxy_handlers() {
    let traps: ProxyTraps = unsafe { std::mem::zeroed() };
    unsafe {
        let extra = 0x5678 as *const c_void;
        let a = CreateProxyHandler(&traps, extra);
        let b = CreateProxyHandler(&traps, extra);
        assert!(!a.is_null() && a != b);
        assert!(!GetProxyHandlerFamily().is_null());
        assert_eq!(GetCrossCompartmentWrapper(), GetCrossCompartmentWrapper());
        assert_ne!(GetCrossCompartmentWrapper(), GetSecurityWrapper());
        let w = CreateWrapperProxyHandler(&traps);
        DeleteWrapperProxyHandler(w);
    }
}
