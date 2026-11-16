//! The WindowProxy (HOST_EXOTIC.md, E3b): `NewWindowProxy` over a global, as
//! Servo's `WindowProxy::new` makes it (windowproxy.rs), with a wrapper
//! handler whose missing traps forward to the Window, and `SetWindowProxy`
//! making it the realm's [[GlobalThisValue]] (ECMA-262 §9.3.3).
use mozjs::glue::{
    CreateWrapperProxyHandler, DeleteCompileOptions, GetProxyPrivate, GetWindowProxyClass,
    IsWrapper, NewCompileOptions, NewWindowProxy, ProxyTraps, UncheckedUnwrapObject,
    UnwrapObjectDynamic, UnwrapObjectStatic, RUST_INTERNED_STRING_TO_JSID,
};
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::{
    CheckedUnwrapStatic, CompartmentSpecifier, EnterRealm, IsWindowProxy, JSClass, JSContext,
    JSObject, JSString, JS_AtomizeStringN, JS_DefineProperty, JS_DeprecatedStringHasLatin1Chars,
    JS_GetLatin1StringCharsAndLength, JS_GetPropertyById, JS_GetTwoByteStringCharsAndLength,
    JS_NewContext, JS_NewGlobalObject, OnNewGlobalHookOption, SetWindowProxy, SetWindowProxyClass,
    ToWindowIfWindowProxy, JS, JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL,
    JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::marker::PhantomData;
use std::ptr;

static GLOBAL_CLASS: JSClass = JSClass {
    name: c"Window".as_ptr(),
    flags: JSCLASS_IS_GLOBAL
        | ((JSCLASS_GLOBAL_SLOT_COUNT & JSCLASS_RESERVED_SLOTS_MASK)
            << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ptr::null(),
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

unsafe fn id(cx: *mut JSContext, name: &str) -> JS::PropertyKey {
    let atom = unsafe { JS_AtomizeStringN(cx, name.as_ptr() as *const _, name.len()) };
    let mut key = VoidId();
    unsafe { RUST_INTERNED_STRING_TO_JSID(cx, atom, handle_mut(&mut key)) };
    key
}

unsafe fn text(s: *mut JSString) -> String {
    let mut len = 0;
    unsafe {
        if JS_DeprecatedStringHasLatin1Chars(s) {
            let chars = JS_GetLatin1StringCharsAndLength(ptr::null_mut(), ptr::null(), s, &mut len);
            String::from_utf8_lossy(std::slice::from_raw_parts(chars, len)).into_owned()
        } else {
            let chars =
                JS_GetTwoByteStringCharsAndLength(ptr::null_mut(), ptr::null(), s, &mut len);
            String::from_utf16_lossy(std::slice::from_raw_parts(chars, len))
        }
    }
}

unsafe fn run(cx: *mut JSContext, src: &str) -> JSVal {
    let options = unsafe { NewCompileOptions(cx, c"window_proxy.js".as_ptr(), 1) };
    let mut source = JS::SourceText::<Utf8Unit> {
        _phantom_0: PhantomData,
        units_: src.as_ptr() as *const Utf8Unit,
        length_: src.len() as u32,
        ownsUnits_: false,
    };
    let mut rval = UndefinedValue();
    let ok = unsafe { JS::Evaluate2(cx, options, &mut source, handle_mut(&mut rval)) };
    unsafe { DeleteCompileOptions(options) };
    assert!(ok, "script threw");
    rval
}

/// Servo's `get` trap reads through to the Window; "magic" is the trap's own.
unsafe extern "C" fn get(
    cx: *mut JSContext,
    proxy: JS::HandleObject,
    _receiver: JS::HandleValue,
    key: JS::HandleId,
    vp: JS::MutableHandleValue,
) -> bool {
    unsafe {
        let magic = id(cx, "magic");
        if (*key.ptr).asBits_ == magic.asBits_ {
            *vp.ptr = Int32Value(7);
            return true;
        }
        let mut target = UndefinedValue();
        GetProxyPrivate(*proxy.ptr, &mut target);
        let target: *mut JSObject = target.to_object();
        JS_GetPropertyById(cx, handle(&target), key, vp)
    }
}

#[test]
fn window_proxy_is_the_global_this_value() {
    unsafe {
        let cx = JS_NewContext(32 << 20, ptr::null_mut());
        // script_thread.rs: the glue's class is registered before any window.
        let class = GetWindowProxyClass();
        assert!(!class.is_null());
        SetWindowProxyClass(cx, class);

        let mut options = RealmOptions::default();
        options.creationOptions_.compSpec_ = CompartmentSpecifier::NewCompartmentAndZone;
        // A first realm, so the window's is not realm 0.
        let _other = JS_NewGlobalObject(
            cx,
            &GLOBAL_CLASS,
            ptr::null_mut(),
            OnNewGlobalHookOption::DontFireOnNewGlobalHook,
            &*options,
        );
        let window = JS_NewGlobalObject(
            cx,
            &GLOBAL_CLASS,
            ptr::null_mut(),
            OnNewGlobalHookOption::DontFireOnNewGlobalHook,
            &*options,
        );
        EnterRealm(cx, window);
        let one = Int32Value(1);
        assert!(JS_DefineProperty(
            cx,
            handle(&window),
            c"x".as_ptr(),
            handle(&one),
            0
        ));
        let w = mozjs::jsval::ObjectValue(window);
        assert!(JS_DefineProperty(
            cx,
            handle(&window),
            c"win".as_ptr(),
            handle(&w),
            0
        ));

        let mut traps: ProxyTraps = std::mem::zeroed();
        traps.get = Some(get);
        let handler = CreateWrapperProxyHandler(&traps);
        let proxy = NewWindowProxy(cx, handle(&window), handler);
        assert!(!proxy.is_null());

        // Unwrapping (Wrapper.cpp): a WindowProxy is a wrapper of its Window,
        // and the static and stopping unwraps keep it.
        assert!(IsWindowProxy(proxy));
        assert!(!IsWindowProxy(window));
        assert!(IsWrapper(proxy));
        assert!(!IsWrapper(window));
        assert_eq!(ToWindowIfWindowProxy(proxy), window);
        assert_eq!(ToWindowIfWindowProxy(window), window);
        assert_eq!(UncheckedUnwrapObject(proxy, false), window);
        assert_eq!(UncheckedUnwrapObject(proxy, true), proxy);
        assert_eq!(UnwrapObjectDynamic(proxy, cx, false), window);
        assert_eq!(UnwrapObjectDynamic(proxy, cx, true), proxy);
        assert_eq!(UnwrapObjectStatic(proxy), proxy);
        assert_eq!(CheckedUnwrapStatic(proxy), proxy);

        // The `get` trap runs; the missing traps forward to the Window.
        let mut v = UndefinedValue();
        let magic = id(cx, "magic");
        assert!(JS_GetPropertyById(
            cx,
            handle(&proxy),
            handle(&magic),
            handle_mut(&mut v)
        ));
        assert_eq!(v.to_int32(), 7);

        // Before SetWindowProxy the global this value is the Window itself.
        assert_eq!(run(cx, "this").to_object(), window);
        assert_eq!(
            mozjs::jsapi::js::detail::ToWindowProxyIfWindowSlow(window),
            window
        );

        SetWindowProxy(cx, handle(&window), handle(&proxy));
        assert_eq!(run(cx, "this").to_object(), proxy);
        assert_eq!(run(cx, "globalThis").to_object(), proxy);
        assert_eq!(
            run(cx, "(function () { return this })()").to_object(),
            proxy
        );
        assert_eq!(
            mozjs::jsapi::js::detail::ToWindowProxyIfWindowSlow(window),
            proxy
        );
        assert_eq!(
            mozjs::jsapi::js::detail::ToWindowProxyIfWindowSlow(proxy),
            proxy
        );
        let r = run(
            cx,
            r#"
            [this === win, this.magic, this.x, 'x' in this, Reflect.ownKeys(this).includes('x'),
             Object.getPrototypeOf(this) === Object.getPrototypeOf(win),
             (function () { 'use strict'; return this })(), x, typeof this
            ].join('|')
            "#,
        );
        assert_eq!(text(r.to_string()), "false|7|1|true|true|true||1|object");
    }
}
