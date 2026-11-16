//! The `JSClassOps::resolve` hook as Servo's `resolve_window` uses it: a global
//! whose class defines properties lazily on a lookup miss, reentering JSAPI
//! from the hook (`src/sys/imp/objects.rs` `FacadeHost::resolve`).
use mozjs::glue::{DeleteCompileOptions, NewCompileOptions};
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::{
    CompartmentSpecifier, EnterRealm, JSClass, JSClassOps, JSContext, JSObject,
    JS_DefinePropertyById2, JS_GetPropertyById, JS_NewContext, JS_NewGlobalObject,
    JS_ResolveStandardClass, OnNewGlobalHookOption, JS, JSCLASS_GLOBAL_SLOT_COUNT,
    JSCLASS_IS_GLOBAL, JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::cell::Cell;
use std::marker::PhantomData;
use std::ptr;

thread_local! {
    static LAZY: Cell<usize> = const { Cell::new(0) };
    static CALLS: Cell<u32> = const { Cell::new(0) };
    static NESTED: Cell<Option<bool>> = const { Cell::new(None) };
}

static GLOBAL_OPS: JSClassOps = JSClassOps {
    addProperty: None,
    delProperty: None,
    enumerate: None,
    newEnumerate: None,
    resolve: Some(resolve),
    mayResolve: None,
    finalize: None,
    call: None,
    construct: None,
    trace: None,
};

static GLOBAL_CLASS: JSClass = JSClass {
    name: c"LazyGlobal".as_ptr(),
    flags: JSCLASS_IS_GLOBAL
        | ((JSCLASS_GLOBAL_SLOT_COUNT & JSCLASS_RESERVED_SLOTS_MASK)
            << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: &GLOBAL_OPS,
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

fn handle<T>(v: &T) -> JS::Handle<T> {
    JS::Handle {
        _phantom_0: PhantomData,
        ptr: v,
    }
}

fn handle_mut<T>(v: &mut T) -> JS::MutableHandle<T> {
    JS::MutableHandle {
        _phantom_0: PhantomData,
        ptr: v,
    }
}

/// Defines `lazy = 7` when asked for it, after a nested lookup of the same
/// id (which must not call the hook again), and lets the standard classes
/// resolve as Servo's hook does.
unsafe extern "C" fn resolve(
    cx: *mut JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    resolved: *mut bool,
) -> bool {
    CALLS.with(|c| c.set(c.get() + 1));
    unsafe {
        if !JS_ResolveStandardClass(cx, obj, id, resolved) {
            return false;
        }
        if *resolved {
            return true;
        }
        if (*id.ptr).asBits_ != LAZY.with(Cell::get) {
            return true;
        }
        let mut inner = UndefinedValue();
        let before = CALLS.with(Cell::get);
        assert!(JS_GetPropertyById(cx, obj, id, handle_mut(&mut inner)));
        NESTED.with(|n| {
            n.set(Some(
                CALLS.with(Cell::get) == before && inner.is_undefined(),
            ))
        });
        let value = Int32Value(7);
        if !JS_DefinePropertyById2(cx, obj, id, handle(&value), 0) {
            return false;
        }
        *resolved = true;
    }
    true
}

unsafe fn evaluate(cx: *mut JSContext, text: &str) -> Option<JSVal> {
    let options = unsafe { NewCompileOptions(cx, c"resolve.js".as_ptr(), 1) };
    let mut src = JS::SourceText::<Utf8Unit> {
        _phantom_0: PhantomData,
        units_: text.as_ptr() as *const Utf8Unit,
        length_: text.len() as u32,
        ownsUnits_: false,
    };
    let mut rval = UndefinedValue();
    let ok = unsafe { JS::Evaluate2(cx, options, &mut src, handle_mut(&mut rval)) };
    unsafe { DeleteCompileOptions(options) };
    ok.then_some(rval)
}

unsafe fn truthy(cx: *mut JSContext, text: &str) -> bool {
    let v = unsafe { evaluate(cx, text) }.unwrap_or_else(|| panic!("threw: {text}"));
    v.is_boolean() && v.to_boolean()
}

#[test]
fn resolve_hook_defines_globals_lazily() {
    unsafe {
        let cx = JS_NewContext(32 << 20, ptr::null_mut());
        let mut options = RealmOptions::default();
        options.creationOptions_.compSpec_ = CompartmentSpecifier::NewCompartmentAndZone;
        let global: *mut JSObject = JS_NewGlobalObject(
            cx,
            &GLOBAL_CLASS,
            ptr::null_mut(),
            OnNewGlobalHookOption::DontFireOnNewGlobalHook,
            &*options,
        );
        assert!(!global.is_null());
        EnterRealm(cx, global);
        let name = "lazy";
        let s = mozjs::jsapi::JS_AtomizeStringN(cx, name.as_ptr() as *const _, name.len());
        let mut key = VoidId();
        mozjs::glue::RUST_INTERNED_STRING_TO_JSID(cx, s, handle_mut(&mut key));
        LAZY.with(|l| l.set(key.asBits_));

        // An identifier reference, `typeof`, `in` and a property get all reach
        // the hook on a miss; once defined, `lazy` is an ordinary own property.
        assert!(truthy(cx, "typeof lazy === 'number'"));
        assert_eq!(NESTED.with(Cell::get), Some(true));
        assert_eq!(evaluate(cx, "lazy").unwrap().to_int32(), 7);
        assert!(truthy(cx, "'lazy' in globalThis && globalThis.lazy === 7"));
        let calls = CALLS.with(Cell::get);
        assert!(truthy(cx, "lazy === 7"));
        assert_eq!(
            CALLS.with(Cell::get),
            calls,
            "a resolved property is not resolved again"
        );
        let mut v = UndefinedValue();
        assert!(JS_GetPropertyById(
            cx,
            handle(&global),
            handle(&key),
            handle_mut(&mut v)
        ));
        assert_eq!(v.to_int32(), 7);

        // A name the hook does not resolve stays absent.
        assert!(truthy(
            cx,
            "typeof missing === 'undefined' && !('missing' in globalThis)"
        ));
        assert!(truthy(
            cx,
            "try { missing; false } catch (e) { e instanceof ReferenceError && e.message === 'missing is not defined' }"
        ));
        // Sloppy assignment to an unresolved name creates a global property.
        assert_eq!(evaluate(cx, "other = 3; other").unwrap().to_int32(), 3);
        // Standard classes still resolve through the hook's JS_ResolveStandardClass.
        assert!(truthy(
            cx,
            "typeof Array === 'function' && [1, 2].length === 2"
        ));
    }
}
