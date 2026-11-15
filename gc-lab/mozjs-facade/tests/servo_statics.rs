//! Servo's window_named_properties.rs builds a `static JSClass` that borrows
//! the proxy class hooks, so these `extern const` statics must stay immutable
//! (MOZJS_FACADE.md). This shape failed in the first servo-facade CI run.
use mozjs::jsapi::{JSClass, ProxyClassExtension, ProxyClassOps, ProxyObjectOps};
use std::ptr;

struct Shared(JSClass);
// SAFETY: test-only wrapper; the class is never mutated.
unsafe impl Sync for Shared {}

static CLASS: Shared = Shared(JSClass {
    name: c"WindowProperties".as_ptr(),
    flags: 0,
    cOps: &ProxyClassOps,
    spec: ptr::null(),
    ext: &ProxyClassExtension,
    oOps: &ProxyObjectOps,
});

#[test]
fn proxy_hooks_can_be_borrowed_in_a_static() {
    assert!(!CLASS.0.cOps.is_null());
}

/// The constant Value handles point at fixed cells, as SpiderMonkey's do:
/// Servo passes `UndefinedHandleValue` to `NewProxyObject` and
/// `Handle::from_raw` unwraps the pointer.
#[test]
fn constant_value_handles_point_at_values() {
    use mozjs::jsapi::JS::{
        FalseHandleValue, NullHandleValue, TrueHandleValue, UndefinedHandleValue,
    };
    unsafe {
        assert!((*UndefinedHandleValue.ptr).is_undefined());
        assert!((*NullHandleValue.ptr).is_null());
        assert!((*TrueHandleValue.ptr).to_boolean());
        assert!((*FalseHandleValue.ptr).is_boolean());
        assert!(!(*FalseHandleValue.ptr).to_boolean());
    }
}
