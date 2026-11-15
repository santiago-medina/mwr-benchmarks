//! The context cluster (`src/sys/imp/context.rs`): the calls `JSEngine::init`
//! and `Runtime::new` make, plus stack rooting over the `RootingContext`
//! prefix of the facade's context. `JSEngine::init` runs once per process, so
//! everything lives in one test.
use mozjs::jsapi::{JSGCParamKey, JS_GetGCParameter, JS_GetRuntime, JS_SetGCParameter};
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::{JSEngine, Runtime};

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
    drop(runtime);
    drop(engine);
}
