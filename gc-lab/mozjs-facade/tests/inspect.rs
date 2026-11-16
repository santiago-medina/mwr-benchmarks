//! Inspection and JSON as Servo's console previews its arguments: builtin
//! class, Map size and entries, array length, function id and arity,
//! ValueToSource, JS_Stringify and JS_ParseJSON (`src/sys/imp/inspect.rs`).
use mozjs::glue::{DeleteCompileOptions, NewCompileOptions};
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::ESClass;
use mozjs::jsapi::{
    CompartmentSpecifier, EnterRealm, JSClass, JSContext, JSObject, JSString,
    JS_ClearPendingException, JS_DeprecatedStringHasLatin1Chars, JS_GetElement,
    JS_GetFunctionArity, JS_GetFunctionDisplayId, JS_GetFunctionId,
    JS_GetLatin1StringCharsAndLength, JS_GetTwoByteStringCharsAndLength, JS_IsExceptionPending,
    JS_NewContext, JS_NewGlobalObject, JS_ParseJSON, JS_ParseJSON2, JS_Stringify,
    JS_ValueToFunction, JS_ValueToSource, OnNewGlobalHookOption, JS, JSCLASS_GLOBAL_SLOT_COUNT,
    JSCLASS_IS_GLOBAL, JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT,
};
use mozjs::jsval::{Int32Value, JSVal, NullValue, UndefinedValue};
use mozjs::rust::RealmOptions;
use std::ffi::c_void;
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

unsafe fn evaluate(cx: *mut JSContext, text: &str) -> JSVal {
    let options = unsafe { NewCompileOptions(cx, c"test.js".as_ptr(), 1) };
    let mut src = JS::SourceText {
        _phantom_0: PhantomData,
        units_: text.as_ptr() as *const Utf8Unit,
        length_: text.len() as u32,
        ownsUnits_: false,
    };
    let mut rval = UndefinedValue();
    let ok = unsafe { JS::Evaluate2(cx, options, &mut src, handle_mut(&mut rval)) };
    unsafe { DeleteCompileOptions(options) };
    assert!(ok, "evaluation of {text:?} threw");
    rval
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

unsafe fn class_of(cx: *mut JSContext, script: &str) -> ESClass {
    let v = unsafe { evaluate(cx, script) };
    let obj: *mut JSObject = v.to_object();
    let mut cls = ESClass::Other;
    assert!(unsafe { mozjs::jsapi::GetBuiltinClass(cx, handle(&obj), &mut cls) });
    cls
}

unsafe fn source_of(cx: *mut JSContext, script: &str) -> String {
    let v = unsafe { evaluate(cx, script) };
    unsafe { text(JS_ValueToSource(cx, handle(&v))) }
}

unsafe extern "C" fn collect(chars: *const u16, len: u32, data: *mut c_void) -> bool {
    let out = unsafe { &mut *data.cast::<Vec<u16>>() };
    out.extend_from_slice(unsafe { std::slice::from_raw_parts(chars, len as usize) });
    true
}

unsafe fn stringify(cx: *mut JSContext, mut v: JSVal, space: JSVal) -> Option<String> {
    let mut out = Vec::<u16>::new();
    let replacer: *mut JSObject = ptr::null_mut();
    let ok = unsafe {
        JS_Stringify(
            cx,
            handle_mut(&mut v),
            handle(&replacer),
            handle(&space),
            Some(collect),
            (&mut out as *mut Vec<u16>).cast(),
        )
    };
    assert!(ok);
    (!out.is_empty()).then(|| String::from_utf16(&out).unwrap())
}

#[test]
fn console_inspection_and_json() {
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

        // An embedder's global owns `document`: the lab's built-in one is
        // not injected.
        let t = evaluate(cx, "typeof document");
        assert_eq!(text(t.to_string()), "undefined");

        // Builtin classes from internal slots.
        assert_eq!(class_of(cx, "({})"), ESClass::Object);
        assert_eq!(class_of(cx, "[1, 2]"), ESClass::Array);
        assert_eq!(class_of(cx, "new Map"), ESClass::Map);
        assert_eq!(class_of(cx, "new Set"), ESClass::Set);
        assert_eq!(class_of(cx, "(function () {})"), ESClass::Function);
        assert_eq!(class_of(cx, "new Error('x')"), ESClass::Error);
        assert_eq!(class_of(cx, "new Date(0)"), ESClass::Date);
        assert_eq!(class_of(cx, "/a/"), ESClass::RegExp);
        assert_eq!(class_of(cx, "Object(1)"), ESClass::Number);
        assert_eq!(class_of(cx, "new WeakMap"), ESClass::Other);
        assert_eq!(class_of(cx, "new Map().entries()"), ESClass::MapIterator);
        assert_eq!(
            class_of(cx, "(function () { return arguments })()"),
            ESClass::Arguments
        );

        // Map size and entries, iterated as console.rs does.
        let map = evaluate(cx, "new Map([['a', 1], ['b', 2]])").to_object();
        assert_eq!(JS::MapSize(cx, handle(&map)), 2);
        let mut entries = UndefinedValue();
        assert!(JS::MapEntries(cx, handle(&map), handle_mut(&mut entries)));
        assert_eq!(class_of_value(cx, entries), ESClass::MapIterator);

        // Array length, and an element.
        let array = evaluate(cx, "[10, 20, 30]").to_object();
        let mut length = 0;
        assert!(JS::GetArrayLength(cx, handle(&array), &mut length));
        assert_eq!(length, 3);
        let mut element = UndefinedValue();
        assert!(JS_GetElement(
            cx,
            handle(&array),
            1,
            handle_mut(&mut element)
        ));
        assert_eq!(element.to_int32(), 20);
        let like = evaluate(cx, "({length: 2.5})").to_object();
        assert!(JS::GetArrayLength(cx, handle(&like), &mut length));
        assert_eq!(length, 2);

        // Function id, display id and arity; a non-callable throws.
        let f = evaluate(cx, "(function adder(a, b) {})");
        let fun = JS_ValueToFunction(cx, handle(&f));
        assert!(!fun.is_null());
        let mut name: *mut JSString = ptr::null_mut();
        assert!(JS_GetFunctionId(cx, handle(&fun), handle_mut(&mut name)));
        assert_eq!(text(name), "adder");
        assert!(JS_GetFunctionDisplayId(
            cx,
            handle(&fun),
            handle_mut(&mut name)
        ));
        assert_eq!(text(name), "adder");
        assert_eq!(JS_GetFunctionArity(fun), 2);
        let anon = evaluate(cx, "[function () {}][0]");
        let anon = JS_ValueToFunction(cx, handle(&anon));
        assert!(JS_GetFunctionId(cx, handle(&anon), handle_mut(&mut name)));
        assert!(name.is_null());
        let not_fn = Int32Value(3);
        assert!(JS_ValueToFunction(cx, handle(&not_fn)).is_null());
        assert!(JS_IsExceptionPending(cx));
        JS_ClearPendingException(cx);

        // ValueToSource, SpiderMonkey's shapes.
        assert_eq!(source_of(cx, "undefined"), "(void 0)");
        assert_eq!(source_of(cx, "-0"), "-0");
        assert_eq!(source_of(cx, "'a\"b\\n'"), "\"a\\\"b\\n\"");
        assert_eq!(source_of(cx, "Symbol('s')"), "Symbol(\"s\")");
        assert_eq!(source_of(cx, "[1, 'x']"), "[1, \"x\"]");
        assert_eq!(source_of(cx, "({a: 1, b: [2]})"), "({\"a\":1, \"b\":[2]})");

        // JS_Stringify: indented, and undefined writes nothing.
        let obj = evaluate(cx, "({a: [1, 2], b: 'x'})");
        assert_eq!(
            stringify(cx, obj, Int32Value(2)).unwrap(),
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": \"x\"\n}"
        );
        assert_eq!(stringify(cx, UndefinedValue(), NullValue()), None);

        // JS_ParseJSON over UTF-16 and Latin-1; a syntax error throws.
        let wide: Vec<u16> = "{\"k\": [true, null]}".encode_utf16().collect();
        let mut parsed = UndefinedValue();
        assert!(JS_ParseJSON(
            cx,
            wide.as_ptr(),
            wide.len() as u32,
            handle_mut(&mut parsed)
        ));
        assert_eq!(
            stringify(cx, parsed, UndefinedValue()).unwrap(),
            "{\"k\":[true,null]}"
        );
        let narrow = b"[1,2";
        assert!(!JS_ParseJSON2(
            cx,
            narrow.as_ptr(),
            narrow.len() as u32,
            handle_mut(&mut parsed)
        ));
        assert!(JS_IsExceptionPending(cx));
        JS_ClearPendingException(cx);
    }
}

unsafe fn class_of_value(cx: *mut JSContext, v: JSVal) -> ESClass {
    let obj: *mut JSObject = v.to_object();
    let mut cls = ESClass::Other;
    assert!(unsafe { mozjs::jsapi::GetBuiltinClass(cx, handle(&obj), &mut cls) });
    cls
}
