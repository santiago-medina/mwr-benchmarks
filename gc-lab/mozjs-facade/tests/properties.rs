//! Property access by id, descriptors, id vectors, linear strings and calls
//! as Servo's bindings and DOM proxy handlers run them
//! (`src/sys/imp/props.rs`).
use mozjs::glue::{
    AppendToIdVector, CreateRootedIdVector, DestroyRootedIdVector, GetIdVectorAddress,
    SetDataPropertyDescriptor, SliceRootedIdVector, RUST_INTERNED_STRING_TO_JSID,
};
use mozjs::jsapi::glue::SetPropertyIgnoringNamedGetter;
use mozjs::jsapi::{
    AtomToLinearString, Call, CompartmentSpecifier, EnterRealm, GetCurrentRealmOrNull,
    GetLinearStringCharAt, GetLinearStringLength, GetObjectProto, GetPropertyKeys, IsCallable,
    JSAtom, JSClass, JSContext, JSObject, JSString, JS_AtomizeStringN, JS_CompareStrings,
    JS_DefinePropertyById, JS_DeletePropertyById, JS_FreezeObject, JS_GetFunctionObject,
    JS_GetOwnPropertyDescriptorById, JS_GetPropertyById, JS_GetPropertyDescriptorById,
    JS_HasOwnPropertyById, JS_IdToValue, JS_IsNativeFunction, JS_NewContext, JS_NewFunction,
    JS_NewGlobalObject, JS_NewPlainObject, JS_SetPropertyById, JS_SetPrototype,
    JS_StringEqualsAscii, OnNewGlobalHookOption, StringIsArrayIndex1, ToBooleanSlow, JS,
    JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL, JSCLASS_RESERVED_SLOTS_MASK,
    JSCLASS_RESERVED_SLOTS_SHIFT, JSITER_HIDDEN, JSITER_OWNONLY, JSPROP_ENUMERATE,
    JSPROP_PERMANENT, JSPROP_READONLY,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
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

unsafe fn atom(cx: *mut JSContext, s: &str) -> *mut JSString {
    unsafe { JS_AtomizeStringN(cx, s.as_ptr() as *const _, s.len()) }
}

unsafe fn id(cx: *mut JSContext, s: &str) -> mozjs::jsapi::jsid {
    let mut id = VoidId();
    let s = unsafe { atom(cx, s) };
    unsafe { RUST_INTERNED_STRING_TO_JSID(cx, s, handle_mut(&mut id)) };
    id
}

unsafe fn get(cx: *mut JSContext, obj: *mut JSObject, name: &str) -> JSVal {
    let key = unsafe { id(cx, name) };
    let mut v = UndefinedValue();
    assert!(unsafe { JS_GetPropertyById(cx, handle(&obj), handle(&key), handle_mut(&mut v)) });
    v
}

unsafe fn define(cx: *mut JSContext, obj: *mut JSObject, name: &str, v: JSVal, attrs: u8) -> bool {
    let key = unsafe { id(cx, name) };
    let mut desc = std::mem::MaybeUninit::<JS::PropertyDescriptor>::zeroed();
    let desc = unsafe {
        SetDataPropertyDescriptor(
            handle_mut(&mut *desc.as_mut_ptr()),
            handle(&v),
            attrs as u32,
        );
        desc.assume_init()
    };
    let mut result = JS::ObjectOpResult { code_: usize::MAX };
    assert!(unsafe {
        JS_DefinePropertyById(cx, handle(&obj), handle(&key), handle(&desc), &mut result)
    });
    result.code_ == 0
}

unsafe extern "C" fn add(_cx: *mut JSContext, argc: c_uint, vp: *mut JS::Value) -> bool {
    assert_eq!(argc, 2);
    unsafe {
        let a = (*vp.add(2)).to_int32();
        let b = (*vp.add(3)).to_int32();
        *vp = Int32Value(a + b);
    }
    true
}

#[test]
fn property_ids_descriptors_and_calls() {
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
        assert!(!GetCurrentRealmOrNull(cx).is_null());

        // Array indices in int range become int ids; others are atoms.
        let five = id(cx, "5");
        assert!(five.is_int() && five.to_int() == 5);
        assert!(id(cx, "05").is_string());
        assert!(id(cx, "4294967295").is_string());
        let mut v = UndefinedValue();
        assert!(JS_IdToValue(cx, five, handle_mut(&mut v)));
        assert_eq!(v.to_int32(), 5);
        let mut index = 0;
        let units: Vec<u16> = "123".encode_utf16().collect();
        assert!(StringIsArrayIndex1(units.as_ptr(), 3, &mut index));
        assert_eq!(index, 123);
        assert!(!StringIsArrayIndex1(units.as_ptr(), 0, &mut index));

        // Linear strings and comparisons.
        let abc = atom(cx, "abc");
        let lin = AtomToLinearString(abc as *mut JSAtom);
        assert_eq!(GetLinearStringLength(lin), 3);
        assert_eq!(GetLinearStringCharAt(lin, 1), b'b' as u16);
        let mut order = 0;
        assert!(JS_CompareStrings(cx, abc, atom(cx, "abd"), &mut order));
        assert_eq!(order, -1);
        let mut same = false;
        assert!(JS_StringEqualsAscii(cx, abc, c"abc".as_ptr(), &mut same));
        assert!(same);
        assert!(!ToBooleanSlow(handle(&StringValue(&*atom(cx, "")))));
        assert!(ToBooleanSlow(handle(&Int32Value(-1))));

        // Define, get, set, has, delete by id.
        let proto = JS_NewPlainObject(cx);
        let obj = JS_NewPlainObject(cx);
        assert!(JS_SetPrototype(cx, handle(&obj), handle(&proto)));
        assert!(define(
            cx,
            proto,
            "inherited",
            Int32Value(1),
            JSPROP_ENUMERATE
        ));
        assert!(define(cx, obj, "x", Int32Value(2), JSPROP_ENUMERATE));
        assert!(define(cx, obj, "hidden", Int32Value(3), 0));
        assert!(define(
            cx,
            obj,
            "fixed",
            Int32Value(4),
            JSPROP_READONLY | JSPROP_PERMANENT
        ));
        // Redefining a permanent property with another value is refused.
        assert!(!define(cx, obj, "fixed", Int32Value(5), 0));
        assert_eq!(get(cx, obj, "x").to_int32(), 2);
        assert_eq!(get(cx, obj, "inherited").to_int32(), 1);
        let x = id(cx, "x");
        assert!(JS_SetPropertyById(
            cx,
            handle(&obj),
            handle(&x),
            handle(&Int32Value(9))
        ));
        assert_eq!(get(cx, obj, "x").to_int32(), 9);
        let fixed = id(cx, "fixed");
        assert!(!JS_SetPropertyById(
            cx,
            handle(&obj),
            handle(&fixed),
            handle(&Int32Value(0))
        ));
        mozjs::jsapi::JS_ClearPendingException(cx);
        let mut found = false;
        let inherited = id(cx, "inherited");
        assert!(JS_HasOwnPropertyById(
            cx,
            handle(&obj),
            handle(&inherited),
            &mut found
        ));
        assert!(!found);
        let mut result = JS::ObjectOpResult { code_: usize::MAX };
        assert!(JS_DeletePropertyById(
            cx,
            handle(&obj),
            handle(&fixed),
            &mut result
        ));
        assert_ne!(result.code_, 0);
        let mut proto_out = ptr::null_mut();
        assert!(GetObjectProto(cx, handle(&obj), handle_mut(&mut proto_out)));
        assert_eq!(proto_out, proto);

        // Descriptors, own and inherited.
        let mut desc = std::mem::MaybeUninit::<JS::PropertyDescriptor>::zeroed().assume_init();
        let mut none = true;
        assert!(JS_GetOwnPropertyDescriptorById(
            cx,
            handle(&obj),
            handle(&fixed),
            handle_mut(&mut desc),
            &mut none
        ));
        assert!(!none);
        assert!(desc.hasWritable_() && !desc.writable_() && !desc.configurable_());
        assert_eq!(desc.value_.to_int32(), 4);
        let mut holder = ptr::null_mut();
        assert!(JS_GetPropertyDescriptorById(
            cx,
            handle(&obj),
            handle(&inherited),
            handle_mut(&mut desc),
            handle_mut(&mut holder),
            &mut none
        ));
        assert!(!none && holder == proto);

        // SetPropertyIgnoringNamedGetter with no own property: a fresh data
        // property lands on the receiver.
        let missing = id(cx, "fresh");
        let blank = std::mem::MaybeUninit::<JS::PropertyDescriptor>::zeroed().assume_init();
        let blank_handle = handle(&blank);
        let receiver = ObjectValue(obj);
        let mut result = JS::ObjectOpResult { code_: usize::MAX };
        assert!(SetPropertyIgnoringNamedGetter(
            cx,
            handle(&obj),
            handle(&missing),
            handle(&Int32Value(8)),
            handle(&receiver),
            &blank_handle,
            &mut result
        ));
        assert_eq!(result.code_, 0);
        assert_eq!(get(cx, obj, "fresh").to_int32(), 8);

        // Key enumeration into an id vector.
        let keys = CreateRootedIdVector(cx);
        let address = GetIdVectorAddress(keys);
        let vector = JS::MutableHandleIdVector { ptr: address };
        assert!(GetPropertyKeys(cx, handle(&obj), JSITER_OWNONLY, vector));
        let mut length = 0;
        let ids = std::slice::from_raw_parts(SliceRootedIdVector(keys, &mut length), length);
        assert_eq!(ids.len(), 2); // x, fresh
        assert!(GetPropertyKeys(
            cx,
            handle(&obj),
            JSITER_OWNONLY | JSITER_HIDDEN,
            JS::MutableHandleIdVector { ptr: address }
        ));
        SliceRootedIdVector(keys, &mut length);
        assert_eq!(length, 2 + 4); // + x, hidden, fixed, fresh
        assert!(AppendToIdVector(
            JS::MutableHandleIdVector { ptr: address },
            handle(&five)
        ));
        SliceRootedIdVector(keys, &mut length);
        assert_eq!(length, 7);
        DestroyRootedIdVector(keys);

        // Calls through a JSNative.
        let fun = JS_NewFunction(cx, Some(add), 2, 0, c"add".as_ptr());
        let fun_obj = JS_GetFunctionObject(fun);
        assert!(IsCallable(fun_obj));
        assert!(!IsCallable(obj));
        assert!(JS_IsNativeFunction(fun_obj, Some(add)));
        let args = [Int32Value(20), Int32Value(22)];
        let array = JS::HandleValueArray {
            length_: 2,
            elements_: args.as_ptr(),
        };
        let mut rval = UndefinedValue();
        assert!(Call(
            cx,
            handle(&UndefinedValue()),
            handle(&ObjectValue(fun_obj)),
            &array,
            handle_mut(&mut rval)
        ));
        assert_eq!(rval.to_int32(), 42);

        assert!(JS_FreezeObject(cx, handle(&obj)));
        assert!(!JS_SetPropertyById(
            cx,
            handle(&obj),
            handle(&x),
            handle(&Int32Value(1))
        ));
        mozjs::jsapi::JS_ClearPendingException(cx);
        assert_eq!(get(cx, obj, "x").to_int32(), 9);
    }
}
