//! The host-owned microtask queue as Servo's job_queue.rs drains it
//! (`src/sys/imp/jobs.rs`): embedder tasks are private values, engine promise
//! jobs are `JSMicroTask*` objects run with `RunJSMicroTask`.
use mozjs::glue::{JS_DequeueNextMicroTask, RUST_INTERNED_STRING_TO_JSID};
use mozjs::jsapi::{
    Call, CompartmentSpecifier, EnqueueMicroTask, EnterRealm, GetExecutionGlobalFromJSMicroTask,
    HasAnyMicroTasks, IsJSMicroTask, JSClass, JSContext, JSObject, JS_AtomizeStringN,
    JS_DestroyContext, JS_GetFunctionObject, JS_GetPropertyById, JS_NewContext, JS_NewFunction,
    JS_NewGlobalObject, MaybeGetHostDefinedDataFromJSMicroTask, OnNewGlobalHookOption,
    PrependMicroTask, RunJSMicroTask, ToMaybeWrappedJSMicroTask, JS, JSCLASS_GLOBAL_SLOT_COUNT,
    JSCLASS_IS_GLOBAL, JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT, JS_GC,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, PrivateValue, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::marker::PhantomData;
use std::os::raw::{c_uint, c_void};
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};

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

static SEEN: AtomicI32 = AtomicI32::new(0);

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

unsafe fn call(cx: *mut JSContext, this: JSVal, f: JSVal, args: &[JSVal]) -> JSVal {
    let array = JS::HandleValueArray {
        length_: args.len(),
        elements_: args.as_ptr(),
    };
    let mut rval = UndefinedValue();
    assert!(unsafe { Call(cx, handle(&this), handle(&f), &array, handle_mut(&mut rval)) });
    rval
}

unsafe extern "C" fn record(_cx: *mut JSContext, argc: c_uint, vp: *mut JS::Value) -> bool {
    assert_eq!(argc, 1);
    unsafe {
        SEEN.store((*vp.add(2)).to_int32(), Ordering::SeqCst);
        *vp = UndefinedValue();
    }
    true
}

unsafe fn dequeue(cx: *mut JSContext) -> JSVal {
    let mut task = UndefinedValue();
    unsafe { JS_DequeueNextMicroTask(cx, handle_mut(&mut task)) };
    task
}

#[test]
fn promise_jobs_and_embedder_tasks_share_one_queue() {
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
        assert!(!HasAnyMicroTasks(cx));

        // Promise.resolve(41).then(record): one reaction job, queued at once.
        let promise_ctor = get(cx, global, "Promise");
        let resolve = get(cx, promise_ctor.to_object(), "resolve");
        let promise = call(cx, promise_ctor, resolve, &[Int32Value(41)]);
        let then = get(cx, promise.to_object(), "then");
        let handler = JS_GetFunctionObject(JS_NewFunction(cx, Some(record), 1, 0, c"r".as_ptr()));
        call(cx, promise, then, &[ObjectValue(handler)]);
        assert!(HasAnyMicroTasks(cx));

        // Embedder tasks around it: one prepended, one appended.
        let (a, b) = (1usize, 2usize);
        assert!(PrependMicroTask(
            cx,
            &PrivateValue(&a as *const usize as *const c_void)
        ));
        assert!(EnqueueMicroTask(
            cx,
            &PrivateValue(&b as *const usize as *const c_void)
        ));

        let first = dequeue(cx);
        assert!(!IsJSMicroTask(&first));
        assert_eq!(first.to_private(), &a as *const usize as *const c_void);

        let job = dequeue(cx);
        assert!(IsJSMicroTask(&job));
        let task = ToMaybeWrappedJSMicroTask(&job);
        assert_eq!(GetExecutionGlobalFromJSMicroTask(task), global);
        let (mut incumbent, mut data) = (global, global);
        assert!(MaybeGetHostDefinedDataFromJSMicroTask(
            task,
            handle_mut(&mut incumbent),
            handle_mut(&mut data)
        ));
        assert!(incumbent.is_null() && data.is_null());
        // A collection between dequeue and run must keep the job alive.
        JS_GC(cx, mozjs::jsapi::GCReason::API);
        assert_eq!(SEEN.load(Ordering::SeqCst), 0);
        assert!(RunJSMicroTask(cx, handle(&task)));
        assert_eq!(SEEN.load(Ordering::SeqCst), 41);
        // A job runs once.
        assert!(!RunJSMicroTask(cx, handle(&task)));

        let last = dequeue(cx);
        assert_eq!(last.to_private(), &b as *const usize as *const c_void);
        assert!(!HasAnyMicroTasks(cx));
        JS_DestroyContext(cx);
    }
}
