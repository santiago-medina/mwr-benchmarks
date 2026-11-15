//! Global creation as Servo's `create_global_object` does it
//! (components/script_bindings/interface.rs), plus reserved slots, realm
//! entry and compartment selection (`src/sys/imp/objects.rs`).
use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::{
    CompartmentIterResult, CompartmentSpecifier, CurrentGlobalOrNull, EnterRealm,
    GetNonCCWObjectGlobal, GetObjectRealmOrNull, GetRealmGlobalOrNull, IsSharableCompartment,
    IsSystemCompartment, JSClass, JSContext, JS_DestroyContext, JS_FireOnNewGlobalObject,
    JS_IsGlobalObject, JS_IterateCompartments, JS_NewContext, JS_NewGlobalObject,
    JS_NewObjectWithGivenProto, JS_SetReservedSlot, LeaveRealm, OnNewGlobalHookOption, JS,
    JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL, JSCLASS_RESERVED_SLOTS_MASK,
    JSCLASS_RESERVED_SLOTS_SHIFT, JS_GC,
};
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, PrivateValue, UndefinedValue};
use mozjs::rust::RealmOptions;
use mozjs::rust::{get_context_realm, get_object_class, get_object_realm};
use std::os::raw::c_void;
use std::ptr;

static GLOBAL_CLASS: JSClass = JSClass {
    name: c"TestGlobal".as_ptr(),
    flags: JSCLASS_IS_GLOBAL
        | (((JSCLASS_GLOBAL_SLOT_COUNT + 2) & JSCLASS_RESERVED_SLOTS_MASK)
            << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ptr::null(),
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

static THING_CLASS: JSClass = JSClass {
    name: c"Thing".as_ptr(),
    flags: (3 & JSCLASS_RESERVED_SLOTS_MASK) << JSCLASS_RESERVED_SLOTS_SHIFT,
    cOps: ptr::null(),
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

unsafe fn new_global(
    cx: *mut JSContext,
    options: &mozjs::jsapi::RealmOptions,
) -> *mut mozjs::jsapi::JSObject {
    unsafe {
        JS_NewGlobalObject(
            cx,
            &GLOBAL_CLASS,
            ptr::null_mut(),
            OnNewGlobalHookOption::DontFireOnNewGlobalHook,
            options,
        )
    }
}

unsafe extern "C" fn first_sharable(
    _cx: *mut JSContext,
    data: *mut c_void,
    compartment: *mut JS::Compartment,
) -> CompartmentIterResult {
    unsafe {
        if IsSharableCompartment(compartment) && !IsSystemCompartment(compartment) {
            *(data as *mut *mut JS::Compartment) = compartment;
            return CompartmentIterResult::Stop;
        }
    }
    CompartmentIterResult::KeepGoing
}

#[test]
fn globals_realms_and_reserved_slots() {
    unsafe {
        let cx = JS_NewContext(32 << 20, ptr::null_mut());
        let mut options = RealmOptions::default();
        options.creationOptions_.compSpec_ = CompartmentSpecifier::NewCompartmentAndZone;
        let global = new_global(cx, &*options);
        assert!(!global.is_null());
        // Servo's inline readers see the class and realm.
        assert_eq!(get_object_class(global), &GLOBAL_CLASS as *const JSClass);
        let realm = get_object_realm(global);
        assert_eq!(realm, GetObjectRealmOrNull(global));
        assert_eq!(GetRealmGlobalOrNull(realm), global);
        assert!(JS_IsGlobalObject(global));
        // Not entered yet: JS_NewGlobalObject leaves the context's realm alone.
        assert!(CurrentGlobalOrNull(cx).is_null());

        // Reserved slots keep their exact bits (private pointers, int32).
        let private = PrivateValue(0x1000 as *const c_void);
        JS_SetReservedSlot(global, 0, &private);
        let null_private = PrivateValue(ptr::null());
        JS_SetReservedSlot(global, 1, &null_private);
        let mut out: JSVal = UndefinedValue();
        JS_GetReservedSlot(global, 0, &mut out);
        assert_eq!(out.to_private() as usize, 0x1000);
        JS_GetReservedSlot(global, 1, &mut out);
        assert!(out.to_private().is_null());

        // JSAutoRealm.
        let old = EnterRealm(cx, global);
        assert!(old.is_null());
        assert_eq!(get_context_realm(cx), realm);
        assert_eq!(CurrentGlobalOrNull(cx), global);
        let handle = mozjs::jsapi::JS::Handle {
            _phantom_0: Default::default(),
            ptr: &global,
        };
        JS_FireOnNewGlobalObject(cx, handle);

        // An object with a class, reserved slots and the global as prototype.
        let thing = JS_NewObjectWithGivenProto(cx, &THING_CLASS, handle);
        assert_eq!(get_object_class(thing), &THING_CLASS as *const JSClass);
        assert_eq!(GetNonCCWObjectGlobal(thing), global);
        assert!(!JS_IsGlobalObject(thing));
        JS_SetReservedSlot(thing, 2, &Int32Value(-5));
        JS_SetReservedSlot(thing, 0, &ObjectValue(global));
        JS_GetReservedSlot(thing, 2, &mut out);
        assert_eq!(out.to_int32(), -5);

        // A second global sharing the first one's compartment (select_compartment).
        let mut found: *mut JS::Compartment = ptr::null_mut();
        JS_IterateCompartments(
            cx,
            &mut found as *mut _ as *mut c_void,
            Some(first_sharable),
        );
        assert!(!found.is_null());
        let mut shared = RealmOptions::default();
        shared.creationOptions_.compSpec_ = CompartmentSpecifier::ExistingCompartment;
        shared.creationOptions_.__bindgen_anon_1.comp_ = found;
        let second = new_global(cx, &*shared);
        assert_ne!(get_object_realm(second), realm);
        let mut count = 0usize;
        unsafe extern "C" fn counter(
            _cx: *mut JSContext,
            data: *mut c_void,
            _c: *mut JS::Compartment,
        ) -> CompartmentIterResult {
            unsafe { *(data as *mut usize) += 1 };
            CompartmentIterResult::KeepGoing
        }
        JS_IterateCompartments(cx, &mut count as *mut _ as *mut c_void, Some(counter));
        assert_eq!(count, 1, "the second global reused the compartment");

        // Nested realm entry, then back out.
        let inner_old = EnterRealm(cx, second);
        assert_eq!(inner_old, realm);
        assert_eq!(CurrentGlobalOrNull(cx), second);
        LeaveRealm(cx, inner_old);
        assert_eq!(CurrentGlobalOrNull(cx), global);

        // Objects Servo holds survive a collection (C1 keep-alive policy).
        JS_GC(cx, mozjs::jsapi::JS::GCReason::API);
        JS_GetReservedSlot(thing, 0, &mut out);
        assert_eq!(out.to_object(), global);
        assert_eq!(get_object_class(thing), &THING_CLASS as *const JSClass);

        LeaveRealm(cx, old);
        assert!(CurrentGlobalOrNull(cx).is_null());
        JS_DestroyContext(cx);
    }
}
