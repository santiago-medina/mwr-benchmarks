//! JIT-info glue as Servo's WebIDL bindings use it: a DOM getter, setter and
//! method reached through `CallJitGetterOp`, `CallJitSetterOp` and
//! `CallJitMethodOp` (`src/sys/imp/jit.rs`).
use mozjs::glue::{CallJitGetterOp, CallJitMethodOp, CallJitSetterOp};
use mozjs::jsapi::{
    JSContext, JSJitGetterCallArgs, JSJitInfo, JSJitMethodCallArgs, JSJitSetterCallArgs, JSObject,
    JS,
};
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr;

unsafe extern "C" fn getter(
    _cx: *mut JSContext,
    _obj: JS::Handle<*mut JSObject>,
    this: *mut c_void,
    args: JSJitGetterCallArgs,
) -> bool {
    let value = unsafe { *this.cast::<i32>() };
    unsafe { *args._base.ptr = Int32Value(value) };
    true
}

unsafe extern "C" fn setter(
    _cx: *mut JSContext,
    _obj: JS::Handle<*mut JSObject>,
    this: *mut c_void,
    args: JSJitSetterCallArgs,
) -> bool {
    unsafe { *this.cast::<i32>() = (*args._base.ptr).to_int32() };
    true
}

unsafe extern "C" fn method(
    _cx: *mut JSContext,
    _obj: JS::Handle<*mut JSObject>,
    this: *mut c_void,
    args: *const JSJitMethodCallArgs,
) -> bool {
    let args = unsafe { &*args };
    let mut sum = unsafe { *this.cast::<i32>() };
    for i in 0..args.argc_ as usize {
        sum += unsafe { (*args.argv_.add(i)).to_int32() };
    }
    let constructing = args.constructing_();
    // rval() is argv[-2].
    unsafe { *args.argv_.sub(2) = Int32Value(if constructing { -sum } else { sum }) };
    true
}

fn info() -> JSJitInfo {
    unsafe { std::mem::zeroed() }
}

#[test]
fn jit_getter_setter_method() {
    let cx: *mut JSContext = ptr::null_mut();
    let obj: *mut JSObject = ptr::null_mut();
    let this_obj = JS::Handle {
        _phantom_0: PhantomData,
        ptr: &obj,
    };
    let mut native: i32 = 5;
    let this = (&mut native as *mut i32).cast::<c_void>();

    unsafe {
        let mut get_info = info();
        get_info.__bindgen_anon_1.getter = Some(getter);
        let mut vp: [JSVal; 2] = [UndefinedValue(), UndefinedValue()];
        assert!(CallJitGetterOp(
            &get_info,
            cx,
            this_obj,
            this,
            0,
            vp.as_mut_ptr()
        ));
        assert_eq!(vp[0].to_int32(), 5);

        let mut set_info = info();
        set_info.__bindgen_anon_1.setter = Some(setter);
        let mut vp: [JSVal; 3] = [UndefinedValue(), UndefinedValue(), Int32Value(9)];
        assert!(CallJitSetterOp(
            &set_info,
            cx,
            this_obj,
            this,
            1,
            vp.as_mut_ptr()
        ));
        assert_eq!(native, 9);

        let mut method_info = info();
        method_info.__bindgen_anon_1.method = Some(method);
        let mut vp: [JSVal; 4] = [
            UndefinedValue(),
            UndefinedValue(),
            Int32Value(1),
            Int32Value(2),
        ];
        assert!(CallJitMethodOp(
            &method_info,
            cx,
            this_obj,
            this,
            2,
            vp.as_mut_ptr()
        ));
        assert_eq!(vp[0].to_int32(), 12);

        // An info without an op fails rather than calling through null.
        let empty = info();
        assert!(!CallJitGetterOp(
            &empty,
            cx,
            this_obj,
            this,
            0,
            vp.as_mut_ptr()
        ));
    }
}
