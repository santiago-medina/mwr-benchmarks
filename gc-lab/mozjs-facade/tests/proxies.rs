//! DOM proxies made by `NewProxyObject` (src/sys/imp/proxy.rs, HOST_EXOTIC.md):
//! a `ProxyTraps` handler in the shape of Servo's named-properties object,
//! whose traps reenter JSAPI on the expando kept in the proxy private value.
use mozjs::glue::{
    AppendToIdVector, CreateProxyHandler, DeleteCompileOptions, GetProxyHandler,
    GetProxyHandlerExtra, GetProxyPrivate, GetProxyReservedSlot, IsProxyHandlerFamily,
    NewCompileOptions, NewProxyObject, ProxyTraps, SetDataPropertyDescriptor, SetProxyPrivate,
    SetProxyReservedSlot, RUST_INTERNED_STRING_TO_JSID,
};
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::{
    CompartmentSpecifier, EnterRealm, GetPropertyKeys, JSClass, JSContext, JSObject, JSString,
    JS_AtomizeStringN, JS_DefineProperty, JS_DefinePropertyById, JS_DeletePropertyById,
    JS_DeprecatedStringHasLatin1Chars, JS_GetLatin1StringCharsAndLength,
    JS_GetOwnPropertyDescriptorById, JS_GetPropertyById, JS_GetTwoByteStringCharsAndLength,
    JS_NewContext, JS_NewGlobalObject, JS_NewPlainObject, JS_SetImmutablePrototype,
    OnNewGlobalHookOption, JS, JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL,
    JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT, JSITER_HIDDEN, JSITER_OWNONLY,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::marker::PhantomData;
use std::os::raw::c_void;
use std::ptr;

static GLOBAL_CLASS: JSClass = JSClass {
    name: c"TestGlobal".as_ptr(),
    flags: JSCLASS_IS_GLOBAL
        | ((JSCLASS_GLOBAL_SLOT_COUNT & JSCLASS_RESERVED_SLOTS_MASK)
            << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ptr::null(),
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

/// One reserved slot, as a DOM reflector's native pointer.
static PROXY_CLASS: JSClass = JSClass {
    name: c"WindowProperties".as_ptr(),
    flags: (1 & JSCLASS_RESERVED_SLOTS_MASK) << JSCLASS_RESERVED_SLOTS_SHIFT,
    cOps: ptr::null(),
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

/// A failure code for `ObjectOpResult` (`JSMSG_CANT_DELETE`).
const FAIL: usize = 10;

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

unsafe fn id(cx: *mut JSContext, s: &str) -> mozjs::jsapi::jsid {
    let mut id = VoidId();
    let atom = unsafe { JS_AtomizeStringN(cx, s.as_ptr() as *const _, s.len()) };
    unsafe { RUST_INTERNED_STRING_TO_JSID(cx, atom, handle_mut(&mut id)) };
    id
}

unsafe fn text(s: *mut JSString) -> String {
    let mut len = 0;
    unsafe {
        if JS_DeprecatedStringHasLatin1Chars(s) {
            let chars = JS_GetLatin1StringCharsAndLength(ptr::null_mut(), ptr::null(), s, &mut len);
            std::slice::from_raw_parts(chars, len)
                .iter()
                .map(|&b| b as char)
                .collect()
        } else {
            let chars =
                JS_GetTwoByteStringCharsAndLength(ptr::null_mut(), ptr::null(), s, &mut len);
            String::from_utf16(std::slice::from_raw_parts(chars, len)).unwrap()
        }
    }
}

unsafe fn evaluate(cx: *mut JSContext, src: &str) -> String {
    let options = unsafe { NewCompileOptions(cx, c"proxies.js".as_ptr(), 1) };
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
    unsafe { text(rval.to_string()) }
}

unsafe fn expando(proxy: JS::HandleObject) -> *mut JSObject {
    let mut v = UndefinedValue();
    unsafe { GetProxyPrivate(*proxy.ptr, &mut v) };
    v.to_object()
}

unsafe fn is_named(cx: *mut JSContext, key: JS::HandleId) -> bool {
    let named = unsafe { id(cx, "named") };
    unsafe { (*key.ptr).asBits_ == named.asBits_ }
}

/// "named" is a supported property name (42, configurable, not enumerable);
/// everything else lives on the expando.
unsafe extern "C" fn get_own(
    cx: *mut JSContext,
    proxy: JS::HandleObject,
    key: JS::HandleId,
    desc: JS::MutableHandle<JS::PropertyDescriptor>,
    is_none: *mut bool,
) -> bool {
    unsafe {
        if is_named(cx, key) {
            let v = Int32Value(42);
            SetDataPropertyDescriptor(desc, handle(&v), 0);
            *is_none = false;
            return true;
        }
        let e = expando(proxy);
        JS_GetOwnPropertyDescriptorById(cx, handle(&e), key, desc, is_none)
    }
}

unsafe extern "C" fn define(
    cx: *mut JSContext,
    proxy: JS::HandleObject,
    key: JS::HandleId,
    desc: JS::Handle<JS::PropertyDescriptor>,
    result: *mut JS::ObjectOpResult,
) -> bool {
    unsafe {
        if is_named(cx, key) {
            (*result).code_ = FAIL;
            return true;
        }
        let e = expando(proxy);
        JS_DefinePropertyById(cx, handle(&e), key, desc, result)
    }
}

unsafe extern "C" fn own_keys(
    cx: *mut JSContext,
    proxy: JS::HandleObject,
    props: JS::MutableHandleIdVector,
) -> bool {
    unsafe {
        let named = id(cx, "named");
        let e = expando(proxy);
        AppendToIdVector(props, handle(&named))
            && GetPropertyKeys(cx, handle(&e), JSITER_OWNONLY | JSITER_HIDDEN, props)
    }
}

unsafe extern "C" fn delete(
    cx: *mut JSContext,
    proxy: JS::HandleObject,
    key: JS::HandleId,
    result: *mut JS::ObjectOpResult,
) -> bool {
    unsafe {
        let e = expando(proxy);
        JS_DeletePropertyById(cx, handle(&e), key, result)
    }
}

unsafe extern "C" fn prevent_extensions(
    _cx: *mut JSContext,
    _proxy: JS::HandleObject,
    result: *mut JS::ObjectOpResult,
) -> bool {
    unsafe { (*result).code_ = FAIL };
    true
}

unsafe extern "C" fn is_extensible(
    _cx: *mut JSContext,
    _proxy: JS::HandleObject,
    extensible: *mut bool,
) -> bool {
    unsafe { *extensible = true };
    true
}

#[test]
fn named_properties_proxy_runs_its_traps() {
    unsafe {
        let cx = JS_NewContext(32 << 20, ptr::null_mut());
        let mut options = RealmOptions::default();
        options.creationOptions_.compSpec_ = CompartmentSpecifier::NewCompartmentAndZone;
        let global = JS_NewGlobalObject(
            cx,
            &GLOBAL_CLASS,
            ptr::null_mut(),
            OnNewGlobalHookOption::DontFireOnNewGlobalHook,
            &*options,
        );
        EnterRealm(cx, global);

        let mut traps: ProxyTraps = std::mem::zeroed();
        traps.getOwnPropertyDescriptor = Some(get_own);
        traps.defineProperty = Some(define);
        traps.ownPropertyKeys = Some(own_keys);
        traps.delete_ = Some(delete);
        traps.preventExtensions = Some(prevent_extensions);
        traps.isExtensible = Some(is_extensible);
        let extra = 0x1234usize as *const c_void;
        let handler = CreateProxyHandler(&traps, extra);

        let proto = JS_NewPlainObject(cx);
        let expando_obj = JS_NewPlainObject(cx);
        let private = ObjectValue(expando_obj);
        let proxy = NewProxyObject(cx, handler, handle(&private), proto, &PROXY_CLASS, false);
        assert!(!proxy.is_null());

        // Handler, extra, family, private and reserved slots.
        assert_eq!(GetProxyHandler(proxy), handler);
        assert_eq!(GetProxyHandlerExtra(proxy), extra);
        assert!(IsProxyHandlerFamily(proxy));
        assert!(!IsProxyHandlerFamily(proto));
        let slot = Int32Value(7);
        SetProxyReservedSlot(proxy, 0, &slot);
        let mut v = UndefinedValue();
        GetProxyReservedSlot(proxy, 0, &mut v);
        assert_eq!(v.to_int32(), 7);
        let other = JS_NewPlainObject(cx);
        SetProxyPrivate(proxy, &ObjectValue(other));
        GetProxyPrivate(proxy, &mut v);
        assert_eq!(v.to_object(), other);
        SetProxyPrivate(proxy, &private);

        // window_named_properties.rs: an immutable prototype.
        let mut succeeded = false;
        assert!(JS_SetImmutablePrototype(cx, handle(&proxy), &mut succeeded));
        assert!(succeeded);

        // A JSAPI read goes through getOwnPropertyDescriptor.
        let named = id(cx, "named");
        assert!(JS_GetPropertyById(
            cx,
            handle(&proxy),
            handle(&named),
            handle_mut(&mut v)
        ));
        assert_eq!(v.to_int32(), 42);

        let p = ObjectValue(proxy);
        assert!(JS_DefineProperty(
            cx,
            handle(&global),
            c"p".as_ptr(),
            handle(&p),
            0
        ));
        let pr = ObjectValue(proto);
        assert!(JS_DefineProperty(
            cx,
            handle(&global),
            c"proto".as_ptr(),
            handle(&pr),
            0
        ));
        let r = evaluate(
            cx,
            r#"
            p.x = 5; proto.inherited = 'i';
            var d = Object.getOwnPropertyDescriptor(p, 'named');
            [p.named, 'named' in p, p.x, p.inherited, d.enumerable, d.configurable, d.writable,
             Reflect.ownKeys(p).join(), Reflect.defineProperty(p, 'named', { value: 1 }),
             delete p.x, 'x' in p, Reflect.preventExtensions(p), Object.isExtensible(p),
             Object.getPrototypeOf(p) === proto, Reflect.setPrototypeOf(p, {}), typeof p
            ].join('|')
            "#,
        );
        assert_eq!(
            r,
            "42|true|5|i|false|true|true|named,x|false|true|false|false|true|true|false|object"
        );
        // Object.keys skips the non-enumerable supported name.
        let r = evaluate(cx, "p.y = 1; String(Object.keys(p))");
        assert_eq!(r, "y");
    }
}
