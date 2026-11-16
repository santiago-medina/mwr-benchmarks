//! Classic scripts as Servo's `evaluate_js_on_global` runs them: compile
//! options, compile, execute, script privates, the hidden scripted caller and
//! the error report of an uncaught exception, plus the unforgeable-holder copy
//! of dom/bindings (`src/sys/imp/scripts.rs`, `src/sys/imp/props.rs`).
use mozjs::glue::{
    DeleteCompileOptions, JS_GetScriptPrivate, NewCompileOptions, PendingExceptionStackInfo,
};
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::{
    CompartmentSpecifier, EnterRealm, JSClass, JSContext, JSErrorReport, JSExnType, JSObject,
    JSScript, JS_ClearPendingException, JS_ErrorFromException, JS_ExecuteScript,
    JS_GetPendingException, JS_GetPropertyById, JS_InitializePropertiesFromCompatibleNativeObject,
    JS_IsExceptionPending, JS_NewContext, JS_NewGlobalObject, JS_NewObjectWithoutMetadata,
    OnNewGlobalHookOption, JS, JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL,
    JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT,
};
use mozjs::jsid::VoidId;
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::ffi::{c_char, c_void, CStr};
use std::marker::PhantomData;
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

static PLAIN_CLASS: JSClass = JSClass {
    name: c"Holder".as_ptr(),
    flags: 0,
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

fn source(text: &str) -> JS::SourceText<Utf8Unit> {
    JS::SourceText {
        _phantom_0: PhantomData,
        units_: text.as_ptr() as *const Utf8Unit,
        length_: text.len() as u32,
        ownsUnits_: false,
    }
}

unsafe fn compile(cx: *mut JSContext, text: &str) -> *mut JSScript {
    let options = unsafe { NewCompileOptions(cx, c"test.js".as_ptr(), 1) };
    let mut src = source(text);
    let script = unsafe { JS::Compile1(cx, options, &mut src) };
    unsafe { DeleteCompileOptions(options) };
    script
}

unsafe fn evaluate(cx: *mut JSContext, text: &str) -> Option<JSVal> {
    let options = unsafe { NewCompileOptions(cx, c"test.js".as_ptr(), 1) };
    let mut src = source(text);
    let mut rval = UndefinedValue();
    let ok = unsafe { JS::Evaluate2(cx, options, &mut src, handle_mut(&mut rval)) };
    unsafe { DeleteCompileOptions(options) };
    ok.then_some(rval)
}

unsafe fn take_exception(cx: *mut JSContext) -> *mut JSObject {
    let mut exc = UndefinedValue();
    assert!(unsafe { JS_GetPendingException(cx, handle_mut(&mut exc)) });
    unsafe { JS_ClearPendingException(cx) };
    assert!(exc.is_object());
    exc.to_object()
}

unsafe extern "C" fn store(ptr: *const c_char, len: usize, target: *mut c_void) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    let target = unsafe { &mut *target.cast::<Option<String>>() };
    *target = Some(String::from_utf8(bytes.to_vec()).unwrap());
}

/// PendingExceptionStackInfo's (message, filename, line, column, exception).
unsafe fn stack_info(cx: *mut JSContext) -> Option<(String, Option<String>, u32, u32, JSVal)> {
    let (mut message, mut filename) = (None::<String>, None::<String>);
    let (mut line, mut col) = (u32::MAX, u32::MAX);
    let mut exc = UndefinedValue();
    let ok = unsafe {
        PendingExceptionStackInfo(
            cx,
            Some(store),
            (&mut message as *mut Option<String>).cast(),
            (&mut filename as *mut Option<String>).cast(),
            &mut line,
            &mut col,
            handle_mut(&mut exc),
        )
    };
    ok.then(|| (message.unwrap(), filename, line, col, exc))
}

/// (message, filename, exnType) of an error object's report.
unsafe fn report(cx: *mut JSContext, obj: *mut JSObject) -> Option<(String, String, i16)> {
    let mut borrowed = JS::BorrowedErrorReport {
        owner_: JS::Rooted::new_unrooted(ptr::null_mut()),
        report_: ptr::null_mut(),
    };
    if !unsafe { JS_ErrorFromException(cx, handle(&obj), &mut borrowed) } {
        return None;
    }
    assert_eq!(borrowed.owner_.data, obj);
    let r: &JSErrorReport = unsafe { &*borrowed.report_ };
    let text = |p| unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
    Some((
        text(r._base.message_.data_),
        text(r._base.filename.data_),
        r.exnType,
    ))
}

unsafe fn get(cx: *mut JSContext, obj: *mut JSObject, name: &str) -> JSVal {
    let mut key = VoidId();
    let s = unsafe { mozjs::jsapi::JS_AtomizeStringN(cx, name.as_ptr() as *const _, name.len()) };
    unsafe { mozjs::glue::RUST_INTERNED_STRING_TO_JSID(cx, s, handle_mut(&mut key)) };
    let mut v = UndefinedValue();
    assert!(unsafe { JS_GetPropertyById(cx, handle(&obj), handle(&key), handle_mut(&mut v)) });
    v
}

#[test]
fn compile_execute_and_error_reports() {
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

        // Compile, then execute: the completion value comes back and global
        // declarations persist for later scripts.
        let script = compile(cx, "var a = 40; a + 2");
        assert!(!script.is_null());
        let mut rval = UndefinedValue();
        assert!(JS_ExecuteScript(cx, handle(&script), handle_mut(&mut rval)));
        assert_eq!(rval.to_int32(), 42);
        assert_eq!(evaluate(cx, "a").unwrap().to_int32(), 40);

        // Script privates round-trip.
        JS::SetScriptPrivate(script, &Int32Value(7));
        let mut private = UndefinedValue();
        JS_GetScriptPrivate(script, handle_mut(&mut private));
        assert_eq!(private.to_int32(), 7);

        JS::HideScriptedCaller(cx);
        JS::UnhideScriptedCaller(cx);

        // An early error fails Compile with a pending SyntaxError.
        assert!(compile(cx, "var = ;").is_null());
        let exc = take_exception(cx);
        let (_, filename, kind) = report(cx, exc).unwrap();
        assert_eq!(filename, "");
        assert_eq!(kind, JSExnType::JSEXN_SYNTAXERR as i16);

        // A runtime throw fails execution; its report carries the message.
        let script = compile(cx, "notDefinedAnywhere");
        assert!(!script.is_null());
        assert!(!JS_ExecuteScript(
            cx,
            handle(&script),
            handle_mut(&mut rval)
        ));
        let exc = take_exception(cx);
        let (message, _, kind) = report(cx, exc).unwrap();
        assert_eq!(message, "notDefinedAnywhere is not defined");
        assert_eq!(kind, JSExnType::JSEXN_REFERENCEERR as i16);
        // The same object gives the same report.
        assert_eq!(report(cx, exc).unwrap().0, message);

        // Thrown non-errors have no report.
        assert!(evaluate(cx, "throw {}").is_none());
        let exc = take_exception(cx);
        assert!(report(cx, exc).is_none());

        // Servo's error_info_from_exception_stack: an error object gives its
        // report and stays the exception; the pending exception is stolen.
        assert!(stack_info(cx).is_none());
        assert!(evaluate(cx, "throw new TypeError('bad thing')").is_none());
        let (message, filename, line, col, exc) = stack_info(cx).unwrap();
        assert_eq!(message, "bad thing");
        assert_eq!(filename.as_deref(), Some(""));
        assert_eq!((line, col), (0, 1));
        assert!(exc.is_object());
        assert_eq!(report(cx, exc.to_object()).unwrap().0, "bad thing");
        assert!(!JS_IsExceptionPending(cx));
        // Non-errors: "uncaught exception: " + ToString, with no filename.
        assert!(evaluate(cx, "throw 'oops'").is_none());
        let (message, filename, _, _, exc) = stack_info(cx).unwrap();
        assert_eq!(message, "uncaught exception: oops");
        assert!(filename.is_none());
        assert!(exc.is_string());
        assert!(evaluate(cx, "throw Symbol('s')").is_none());
        assert_eq!(stack_info(cx).unwrap().0, "uncaught exception: Symbol(s)");
        assert!(evaluate(cx, "throw { toString() { throw 1; } }").is_none());
        assert_eq!(
            stack_info(cx).unwrap().0,
            "uncaught exception: unknown (can't convert to string)"
        );
        assert!(!JS_IsExceptionPending(cx));

        // Unforgeable holders: every own property, accessors included,
        // lands on the fresh object.
        let holder = evaluate(cx, "({ x: 9, get y() { return this.x + 1; } })")
            .unwrap()
            .to_object();
        let dst = JS_NewObjectWithoutMetadata(cx, &PLAIN_CLASS, handle(&ptr::null_mut()));
        assert!(!dst.is_null());
        assert!(JS_InitializePropertiesFromCompatibleNativeObject(
            cx,
            handle(&dst),
            handle(&holder)
        ));
        assert_eq!(get(cx, dst, "x").to_int32(), 9);
        assert_eq!(get(cx, dst, "y").to_int32(), 10);
    }
}

static CALLER_GLOBAL: std::sync::atomic::AtomicPtr<JSObject> =
    std::sync::atomic::AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn record_caller(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let global = unsafe { JS::GetScriptedCallerGlobal(cx) };
    CALLER_GLOBAL.store(global, std::sync::atomic::Ordering::SeqCst);
    unsafe { *vp = UndefinedValue() };
    true
}

#[test]
fn scripted_caller_global_is_the_running_scripts_realm() {
    use mozjs::jsapi::{JS_GetFunctionObject, JS_NewFunction, JS_SetProperty};
    use std::sync::atomic::Ordering;
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

        // No script is running.
        assert!(JS::GetScriptedCallerGlobal(cx).is_null());

        let fun = JS_GetFunctionObject(JS_NewFunction(
            cx,
            Some(record_caller),
            0,
            0,
            c"probe".as_ptr(),
        ));
        let fun_value = mozjs::jsval::ObjectValue(fun);
        assert!(JS_SetProperty(
            cx,
            handle(&global),
            c"probe".as_ptr(),
            handle(&fun_value)
        ));

        // A native called from a script sees that script's global.
        assert!(evaluate(cx, "probe()").is_some());
        assert_eq!(CALLER_GLOBAL.load(Ordering::SeqCst), global);

        // A hidden caller reports none.
        JS::HideScriptedCaller(cx);
        CALLER_GLOBAL.store(global, Ordering::SeqCst);
        assert!(evaluate(cx, "probe()").is_some());
        assert!(CALLER_GLOBAL.load(Ordering::SeqCst).is_null());
        JS::UnhideScriptedCaller(cx);
    }
}

/// MOZJS_FACADE.md "Interpreter guards": on a thread with the script thread's
/// 8 MiB stack, a parentless context runs past the harness guards (call depth
/// 32, 10,000 steps) and runaway recursion is a catchable RangeError.
#[test]
fn web_engine_has_no_step_budget_and_a_native_stack_guard() {
    let run = || unsafe {
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
        let deep = "function f(n) { return n === 0 ? 0 : 1 + f(n - 1); } f(50)";
        assert_eq!(evaluate(cx, deep).unwrap().to_int32(), 50);
        let long = "var s = 0; for (var i = 0; i < 20000; i++) s += 1; s";
        assert_eq!(evaluate(cx, long).unwrap().to_int32(), 20000);
        let runaway = "function g() { return g() + 1; }
            var r = 0;
            try { g(); } catch (e) { r = e instanceof RangeError ? 1 : 2; }
            r";
        assert_eq!(evaluate(cx, runaway).unwrap().to_int32(), 1);
        assert!(!JS_IsExceptionPending(cx));
    };
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
