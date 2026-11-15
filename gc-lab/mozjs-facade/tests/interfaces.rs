//! WebIDL interface setup as Servo's `script_bindings/interface.rs` and
//! `constructor.rs` run it: spec lists, JSNative accessors and functions,
//! constructor/prototype links, immutable prototypes, strings and symbols
//! (`src/sys/imp/natives.rs`).
use mozjs::glue::{RUST_FUNCTION_VALUE_TO_JITINFO, RUST_SYMBOL_TO_JSID};
use mozjs::jsapi::{
    CompartmentSpecifier, EnterRealm, GetRealmObjectPrototype, GetWellKnownSymbol, JSClass,
    JSContext, JSFunctionSpec, JSJitInfo, JSNativeWrapper, JSObject, JSPropertySpec,
    JSPropertySpec_Accessor, JSPropertySpec_AccessorsOrValue,
    JSPropertySpec_AccessorsOrValue_Accessors, JSPropertySpec_Kind, JSPropertySpec_Name,
    JSPropertySpec_ValueWrapper, JSPropertySpec_ValueWrapper_Type,
    JSPropertySpec_ValueWrapper__bindgen_ty_1, JSString, JS_DefineFunctions, JS_DefineProperties,
    JS_DefineProperty5, JS_DefinePropertyById2, JS_DeprecatedStringHasLatin1Chars,
    JS_GetFunctionObject, JS_GetLatin1StringCharsAndLength, JS_GetProperty, JS_GetPrototype,
    JS_GetStringLength, JS_GetTwoByteStringCharsAndLength, JS_HasProperty,
    JS_LinkConstructorAndPrototype, JS_NewContext, JS_NewFunction, JS_NewGlobalObject,
    JS_NewPlainObject, JS_NewStringCopyUTF8N, JS_NewStringCopyZ, JS_SetImmutablePrototype,
    JS_SetPrototype, OnNewGlobalHookOption, JS, JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL,
    JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT, JSFUN_CONSTRUCTOR, JSPROP_ENUMERATE,
    JSPROP_PERMANENT, JSPROP_READONLY,
};
use mozjs::jsapi::{
    JS_ClearPendingException, JS_GetPendingException, JS_IsExceptionPending,
    JS_ResolveStandardClass, JS_SetPendingException,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::marker::PhantomData;
use std::os::raw::c_uint;
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

/// Only its address matters: the facade hands the pointer back.
static INFO_TAG: u64 = 0x1f0;

fn info() -> *const JSJitInfo {
    &INFO_TAG as *const u64 as *const JSJitInfo
}

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

unsafe fn get(cx: *mut JSContext, obj: *mut JSObject, name: &std::ffi::CStr) -> JSVal {
    let mut v = UndefinedValue();
    assert!(unsafe { JS_GetProperty(cx, handle(&obj), name.as_ptr(), handle_mut(&mut v)) });
    v
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

/// A getter that reenters the facade: it creates an object and defines a
/// property on it while the engine that called it is parked.
unsafe extern "C" fn answer_getter(cx: *mut JSContext, argc: c_uint, vp: *mut JSVal) -> bool {
    unsafe {
        assert_eq!(argc, 0);
        let callee = *vp;
        let this = *vp.add(1);
        assert!(callee.is_object() && this.is_object());
        let obj = JS_NewPlainObject(cx);
        assert!(JS_DefineProperty5(
            cx,
            handle(&obj),
            c"x".as_ptr(),
            7,
            JSPROP_ENUMERATE as c_uint
        ));
        // The getter's own name, read through the reentered engine.
        let name = get(cx, callee.to_object(), c"name");
        assert_eq!(text(name.to_string()), "get answer");
        *vp = ObjectValue(obj);
        true
    }
}

unsafe extern "C" fn failing_getter(_cx: *mut JSContext, _argc: c_uint, _vp: *mut JSVal) -> bool {
    false
}

unsafe extern "C" fn throwing_getter(cx: *mut JSContext, _argc: c_uint, _vp: *mut JSVal) -> bool {
    let value = Int32Value(7);
    unsafe { JS_SetPendingException(cx, handle(&value), JS::ExceptionStackBehavior::DoNotCapture) };
    false
}

unsafe extern "C" fn frob(_cx: *mut JSContext, _argc: c_uint, vp: *mut JSVal) -> bool {
    unsafe { *vp = Int32Value(1) };
    true
}

fn spec_string(s: &'static std::ffi::CStr) -> JSPropertySpec_Name {
    JSPropertySpec_Name {
        string_: s.as_ptr(),
    }
}

fn accessor(name: &'static std::ffi::CStr, getter: mozjs::jsapi::JSNative) -> JSPropertySpec {
    JSPropertySpec {
        name: spec_string(name),
        attributes_: JSPROP_ENUMERATE,
        kind_: JSPropertySpec_Kind::NativeAccessor,
        u: JSPropertySpec_AccessorsOrValue {
            accessors: JSPropertySpec_AccessorsOrValue_Accessors {
                getter: JSPropertySpec_Accessor {
                    native: JSNativeWrapper {
                        op: getter,
                        info: info(),
                    },
                },
                setter: JSPropertySpec_Accessor {
                    native: JSNativeWrapper::ZERO,
                },
            },
        },
    }
}

fn int_constant(name: &'static std::ffi::CStr, value: i32) -> JSPropertySpec {
    JSPropertySpec {
        name: spec_string(name),
        attributes_: JSPROP_ENUMERATE | JSPROP_READONLY | JSPROP_PERMANENT,
        kind_: JSPropertySpec_Kind::Value,
        u: JSPropertySpec_AccessorsOrValue {
            value: JSPropertySpec_ValueWrapper {
                type_: JSPropertySpec_ValueWrapper_Type::Int32,
                __bindgen_anon_1: JSPropertySpec_ValueWrapper__bindgen_ty_1 { int32: value },
            },
        },
    }
}

#[test]
fn interface_objects_specs_and_natives() {
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
        assert!(!global.is_null());
        EnterRealm(cx, global);

        // A plain object inherits from the realm's Object.prototype.
        let proto = JS_NewPlainObject(cx);
        let mut found = ptr::null_mut();
        assert!(JS_GetPrototype(cx, handle(&proto), handle_mut(&mut found)));
        assert_eq!(found, GetRealmObjectPrototype(cx));

        // JSFunctionSpec list: a JSNative method and a self-hosted @@iterator.
        let functions = [
            JSFunctionSpec {
                name: spec_string(c"frob"),
                call: JSNativeWrapper {
                    op: Some(frob),
                    info: info(),
                },
                nargs: 2,
                flags: JSPROP_ENUMERATE as u16,
                selfHostedName: ptr::null(),
            },
            JSFunctionSpec {
                name: JSPropertySpec_Name {
                    symbol_: JS::SymbolCode::iterator as usize + 1,
                },
                call: JSNativeWrapper::ZERO,
                nargs: 0,
                flags: 0,
                selfHostedName: c"$ArrayValues".as_ptr(),
            },
            JSFunctionSpec::ZERO,
        ];
        assert!(JS_DefineFunctions(cx, handle(&proto), functions.as_ptr()));
        let method = get(cx, proto, c"frob");
        assert!(method.is_object());
        assert_eq!(RUST_FUNCTION_VALUE_TO_JITINFO(method), info());
        assert_eq!(
            text(get(cx, method.to_object(), c"name").to_string()),
            "frob"
        );
        assert_eq!(get(cx, method.to_object(), c"length").to_int32(), 2);
        assert!(RUST_FUNCTION_VALUE_TO_JITINFO(Int32Value(3)).is_null());

        // JSPropertySpec list: a reentrant native getter and a constant.
        let properties = [
            accessor(c"answer", Some(answer_getter)),
            accessor(c"broken", Some(failing_getter)),
            accessor(c"throwing", Some(throwing_getter)),
            int_constant(c"CONST", 42),
            JSPropertySpec::ZERO,
        ];
        assert!(JS_DefineProperties(cx, handle(&proto), properties.as_ptr()));
        let answer = get(cx, proto, c"answer");
        assert!(answer.is_object());
        assert_eq!(get(cx, answer.to_object(), c"x").to_int32(), 7);
        // The engine came back from the parked slot: a second call works too.
        let again = get(cx, proto, c"answer");
        assert_ne!(again.to_object(), answer.to_object());
        let mut v = UndefinedValue();
        assert!(!JS_GetProperty(
            cx,
            handle(&proto),
            c"broken".as_ptr(),
            handle_mut(&mut v)
        ));
        // False with nothing pending is uncatchable: no exception is left.
        assert!(!JS_IsExceptionPending(cx));
        // A thrown value crosses the engine and comes back pending.
        assert!(!JS_GetProperty(
            cx,
            handle(&proto),
            c"throwing".as_ptr(),
            handle_mut(&mut v)
        ));
        assert!(JS_IsExceptionPending(cx));
        let mut thrown = UndefinedValue();
        assert!(JS_GetPendingException(cx, handle_mut(&mut thrown)));
        assert_eq!(thrown.to_int32(), 7);
        JS_ClearPendingException(cx);
        assert!(!JS_IsExceptionPending(cx));
        assert!(!JS_GetPendingException(cx, handle_mut(&mut thrown)));
        // Standard classes are eager: the resolve hook resolves nothing.
        let id = VoidId();
        let mut resolved = true;
        assert!(JS_ResolveStandardClass(
            cx,
            handle(&proto),
            handle(&id),
            &mut resolved
        ));
        assert!(!resolved);
        assert_eq!(get(cx, proto, c"CONST").to_int32(), 42);
        // Non-configurable: redefining fails.
        assert!(!JS_DefineProperty5(
            cx,
            handle(&proto),
            c"CONST".as_ptr(),
            1,
            0
        ));

        // Constructor and prototype linked both ways.
        let ctor = JS_GetFunctionObject(JS_NewFunction(
            cx,
            Some(frob),
            0,
            JSFUN_CONSTRUCTOR,
            c"Thing".as_ptr(),
        ));
        assert!(!ctor.is_null());
        assert!(JS_LinkConstructorAndPrototype(
            cx,
            handle(&ctor),
            handle(&proto)
        ));
        assert_eq!(get(cx, ctor, c"prototype").to_object(), proto);
        assert_eq!(get(cx, proto, c"constructor").to_object(), ctor);

        // Immutable prototypes (WebIDL global chains).
        let mut succeeded = false;
        assert!(JS_SetImmutablePrototype(cx, handle(&proto), &mut succeeded));
        assert!(succeeded);
        let null: *mut JSObject = ptr::null_mut();
        assert!(!JS_SetPrototype(cx, handle(&proto), handle(&null)));
        let other = JS_NewPlainObject(cx);
        assert!(JS_SetPrototype(cx, handle(&other), handle(&null)));
        let mut found = proto;
        assert!(JS_GetPrototype(cx, handle(&other), handle_mut(&mut found)));
        assert!(found.is_null());
        let mut has = false;
        assert!(JS_HasProperty(
            cx,
            handle(&proto),
            c"toString".as_ptr(),
            &mut has
        ));
        assert!(has);
        assert!(JS_HasProperty(
            cx,
            handle(&other),
            c"toString".as_ptr(),
            &mut has
        ));
        assert!(!has);

        // Strings: Latin-1 and two-byte forms, interning, UTF-8 validation.
        let latin = JS_NewStringCopyZ(cx, c"caf\xe9".as_ptr());
        assert!(JS_DeprecatedStringHasLatin1Chars(latin));
        assert_eq!(text(latin), "café");
        let utf8 = "h\u{e9}llo \u{20ac}";
        let chars = JS::UTF8Chars {
            _base: mozjs::jsapi::mozilla::Range {
                mStart: mozjs::jsapi::mozilla::RangedPtr {
                    mPtr: utf8.as_ptr() as *mut u8,
                    _phantom_0: PhantomData,
                },
                mEnd: mozjs::jsapi::mozilla::RangedPtr {
                    mPtr: utf8.as_ptr().add(utf8.len()) as *mut u8,
                    _phantom_0: PhantomData,
                },
                _phantom_0: PhantomData,
            },
        };
        let wide = JS_NewStringCopyUTF8N(cx, &chars);
        assert!(!JS_DeprecatedStringHasLatin1Chars(wide));
        assert_eq!(JS_GetStringLength(wide), 7);
        assert_eq!(text(wide), utf8);
        assert_eq!(JS_NewStringCopyZ(cx, c"caf\xe9".as_ptr()), latin);

        // Well-known symbols are stable and usable as property keys.
        let iterator = GetWellKnownSymbol(cx, JS::SymbolCode::iterator);
        assert_eq!(iterator, GetWellKnownSymbol(cx, JS::SymbolCode::iterator));
        assert_ne!(
            iterator,
            GetWellKnownSymbol(cx, JS::SymbolCode::toStringTag)
        );
        let mut id = VoidId();
        RUST_SYMBOL_TO_JSID(iterator, handle_mut(&mut id));
        assert!(id.is_symbol());
        let value = Int32Value(5);
        assert!(JS_DefinePropertyById2(
            cx,
            handle(&other),
            handle(&id),
            handle(&value),
            JSPROP_ENUMERATE as c_uint
        ));
    }
}
