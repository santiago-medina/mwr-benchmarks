//! The Promise API (js/public/Promise.h, `src/sys/imp/promises.rs`) along the
//! paths Servo's dom/promise/promise.rs takes: a promise from a native
//! executor, native reaction handlers holding their state in function
//! reserved slots, the original resolving operations and the rejection
//! tracker (HostPromiseRejectionTracker, ECMA-262 §27.2.1.9).
use mozjs::glue::{JS_DequeueNextMicroTask, JS_GetPromiseResult, RUST_INTERNED_STRING_TO_JSID};
use mozjs::jsapi::{
    AddPromiseReactions, CallOriginalPromiseReject, CallOriginalPromiseResolve,
    CallOriginalPromiseThen, CompartmentSpecifier, EnterRealm, FunctionHasNativeReserved,
    GetFunctionNativeReserved, GetPromiseConstructor, GetPromiseIsHandled, GetPromisePrototype,
    GetPromiseState, GetPromiseUserInputEventHandlingState, HasAnyMicroTasks, IsJSMicroTask,
    IsPromiseObject, JSClass, JSContext, JSObject, JS_AtomizeStringN, JS_DestroyContext,
    JS_GetFunctionObject, JS_GetPropertyById, JS_NewContext, JS_NewFunction, JS_NewGlobalObject,
    JS_NewPlainObject, NewFunctionWithReserved, NewPromiseObject, OnNewGlobalHookOption,
    PromiseRejectionHandlingState, PromiseState, PromiseUserInputEventHandlingState, RejectPromise,
    ResolvePromise, RunJSMicroTask, SetAnyPromiseIsHandled, SetFunctionNativeReserved,
    SetPromiseRejectionTrackerCallback, SetPromiseUserInputEventHandlingState,
    ToMaybeWrappedJSMicroTask, JS, JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL,
    JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT, JS_GC,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::os::raw::{c_uint, c_void};
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

thread_local! {
    /// What the native handler saw: (task slot, holder slot is an object, argument).
    static HANDLED: RefCell<Vec<(i32, bool, i32)>> = const { RefCell::new(Vec::new()) };
    /// Rejection tracker events: (promise, handled, state seen from inside).
    static TRACKED: RefCell<Vec<(*mut JSObject, bool, PromiseState)>> =
        const { RefCell::new(Vec::new()) };
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

const NULL: *mut JSObject = ptr::null_mut();

unsafe fn get(cx: *mut JSContext, obj: *mut JSObject, name: &str) -> JSVal {
    let mut key = VoidId();
    unsafe {
        let s = JS_AtomizeStringN(cx, name.as_ptr() as *const _, name.len());
        RUST_INTERNED_STRING_TO_JSID(cx, s, handle_mut(&mut key));
    }
    let mut v = UndefinedValue();
    assert!(unsafe { JS_GetPropertyById(cx, handle(&obj), handle(&key), handle_mut(&mut v)) });
    v
}

unsafe fn new_cx() -> (*mut JSContext, *mut JSObject) {
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
        (cx, global)
    }
}

/// Run every queued microtask (all of them are engine jobs here).
unsafe fn drain(cx: *mut JSContext) {
    unsafe {
        while HasAnyMicroTasks(cx) {
            let mut job = UndefinedValue();
            JS_DequeueNextMicroTask(cx, handle_mut(&mut job));
            assert!(IsJSMicroTask(&job));
            let task = ToMaybeWrappedJSMicroTask(&job);
            assert!(RunJSMicroTask(cx, handle(&task)));
        }
    }
}

unsafe fn result(promise: *mut JSObject) -> JSVal {
    let mut v = UndefinedValue();
    unsafe { JS_GetPromiseResult(handle(&promise), handle_mut(&mut v)) };
    v
}

/// Servo's do-nothing executor (Promise::create_js_promise).
unsafe extern "C" fn do_nothing(_cx: *mut JSContext, _argc: c_uint, vp: *mut JS::Value) -> bool {
    unsafe { *vp = UndefinedValue() };
    true
}

/// Servo's native_handler_callback: the state lives in the callee's slots.
unsafe extern "C" fn native_handler(_cx: *mut JSContext, argc: c_uint, vp: *mut JS::Value) -> bool {
    unsafe {
        let callee = (*vp).to_object();
        let holder = *GetFunctionNativeReserved(callee, 0);
        let task = *GetFunctionNativeReserved(callee, 1);
        let arg = if argc > 0 {
            (*vp.add(2)).to_int32()
        } else {
            -1
        };
        HANDLED.with(|h| {
            h.borrow_mut()
                .push((task.to_int32(), holder.is_object(), arg))
        });
        *vp = UndefinedValue();
    }
    true
}

unsafe extern "C" fn tracker(
    _cx: *mut JSContext,
    muted: bool,
    promise: JS::HandleObject,
    state: PromiseRejectionHandlingState,
    _data: *mut c_void,
) {
    assert!(!muted);
    // The callback may reenter the engine (Servo reads the promise state).
    let seen = unsafe { GetPromiseState(promise) };
    let handled = state == PromiseRejectionHandlingState::Handled;
    TRACKED.with(|t| {
        t.borrow_mut()
            .push((unsafe { *promise.ptr }, handled, seen))
    });
}

unsafe fn native_handler_function(cx: *mut JSContext, task: i32) -> *mut JSObject {
    unsafe {
        let fun = NewFunctionWithReserved(cx, Some(native_handler), 1, 0, ptr::null());
        let obj = JS_GetFunctionObject(fun);
        assert!(FunctionHasNativeReserved(obj));
        assert!(GetFunctionNativeReserved(obj, 0).read().is_undefined());
        let holder = JS_NewPlainObject(cx);
        SetFunctionNativeReserved(obj, 0, &ObjectValue(holder));
        SetFunctionNativeReserved(obj, 1, &Int32Value(task));
        obj
    }
}

#[test]
fn servo_promise_from_a_native_executor_and_native_handlers() {
    unsafe {
        let (cx, global) = new_cx();
        let ctor = GetPromiseConstructor(cx);
        assert_eq!(ObjectValue(ctor), get(cx, global, "Promise"));
        assert_eq!(
            ObjectValue(GetPromisePrototype(cx)),
            get(cx, ctor, "prototype")
        );

        let executor =
            JS_GetFunctionObject(JS_NewFunction(cx, Some(do_nothing), 2, 0, ptr::null()));
        let promise = NewPromiseObject(cx, handle(&executor));
        assert!(!promise.is_null());
        assert!(IsPromiseObject(handle(&promise)));
        assert!(!IsPromiseObject(handle(&global)));
        assert!(!IsPromiseObject(handle(&executor)));
        assert_eq!(
            GetPromiseUserInputEventHandlingState(promise),
            PromiseUserInputEventHandlingState::DontCare
        );
        assert!(SetPromiseUserInputEventHandlingState(
            handle(&promise),
            PromiseUserInputEventHandlingState::HadUserInteractionAtCreation
        ));
        assert_eq!(
            GetPromiseUserInputEventHandlingState(promise),
            PromiseUserInputEventHandlingState::HadUserInteractionAtCreation
        );
        assert_eq!(GetPromiseState(handle(&promise)), PromiseState::Pending);

        let on_fulfilled = native_handler_function(cx, 1);
        let on_rejected = native_handler_function(cx, 2);
        // The holder objects live only in the reserved slots.
        JS_GC(cx, mozjs::jsapi::GCReason::API);
        assert!(AddPromiseReactions(
            cx,
            handle(&promise),
            handle(&on_fulfilled),
            handle(&on_rejected)
        ));
        assert!(!HasAnyMicroTasks(cx));

        assert!(ResolvePromise(
            cx,
            handle(&promise),
            handle(&Int32Value(41))
        ));
        // [[AlreadyResolved]]: later settlements are no-ops (§27.2.1.3).
        assert!(ResolvePromise(cx, handle(&promise), handle(&Int32Value(1))));
        assert!(RejectPromise(cx, handle(&promise), handle(&Int32Value(2))));
        assert_eq!(GetPromiseState(handle(&promise)), PromiseState::Fulfilled);
        assert_eq!(result(promise).to_int32(), 41);

        JS_GC(cx, mozjs::jsapi::GCReason::API);
        drain(cx);
        HANDLED.with(|h| assert_eq!(*h.borrow(), vec![(1, true, 41)]));

        // then() with the original %Promise%: a derived promise.
        let derived =
            CallOriginalPromiseThen(cx, handle(&promise), handle(&on_fulfilled), handle(&NULL));
        assert!(IsPromiseObject(handle(&derived)));
        drain(cx);
        assert_eq!(GetPromiseState(handle(&derived)), PromiseState::Fulfilled);
        assert!(result(derived).is_undefined());
        HANDLED.with(|h| assert_eq!(h.borrow().len(), 2));

        // Reactions on a non-promise throw a TypeError.
        assert!(!AddPromiseReactions(
            cx,
            handle(&global),
            handle(&on_fulfilled),
            handle(&NULL)
        ));
        JS_DestroyContext(cx);
    }
}

#[test]
fn original_resolve_reject_and_the_rejection_tracker() {
    unsafe {
        let (cx, _global) = new_cx();
        SetPromiseRejectionTrackerCallback(cx, Some(tracker), ptr::null_mut());

        // PromiseResolve(%Promise%, x): a promise passes through unchanged.
        let fulfilled = CallOriginalPromiseResolve(cx, handle(&Int32Value(7)));
        assert_eq!(GetPromiseState(handle(&fulfilled)), PromiseState::Fulfilled);
        assert_eq!(result(fulfilled).to_int32(), 7);
        let same = CallOriginalPromiseResolve(cx, handle(&ObjectValue(fulfilled)));
        assert_eq!(same, fulfilled);

        // A promise with no executor, rejected by the embedder: "reject".
        let promise = NewPromiseObject(cx, handle(&NULL));
        assert!(RejectPromise(cx, handle(&promise), handle(&Int32Value(3))));
        assert!(!GetPromiseIsHandled(handle(&promise)));
        TRACKED.with(|t| assert_eq!(*t.borrow(), vec![(promise, false, PromiseState::Rejected)]));

        // Its first handler: "handle".
        let on_rejected = native_handler_function(cx, 2);
        assert!(AddPromiseReactions(
            cx,
            handle(&promise),
            handle(&NULL),
            handle(&on_rejected)
        ));
        assert!(GetPromiseIsHandled(handle(&promise)));
        TRACKED.with(|t| assert_eq!(t.borrow()[1], (promise, true, PromiseState::Rejected)));
        drain(cx);
        HANDLED.with(|h| assert_eq!(*h.borrow(), vec![(2, true, 3)]));

        // Promise.reject(x) with %Promise%, then marked handled by the
        // embedder: no "handle" event, as in SpiderMonkey.
        let rejected = CallOriginalPromiseReject(cx, handle(&Int32Value(9)));
        assert_eq!(GetPromiseState(handle(&rejected)), PromiseState::Rejected);
        assert_eq!(result(rejected).to_int32(), 9);
        assert!(SetAnyPromiseIsHandled(cx, handle(&rejected)));
        assert!(GetPromiseIsHandled(handle(&rejected)));
        TRACKED.with(|t| {
            let t = t.borrow();
            assert_eq!(t.len(), 3);
            assert_eq!(t[2], (rejected, false, PromiseState::Rejected));
        });
        JS_DestroyContext(cx);
    }
}
