//! Stack capture as Servo's console does it (`CapturedJSStack`): with no
//! engine frame stack yet, a capture is a null stack and the SavedFrame
//! accessors answer SpiderMonkey's null-frame defaults (`src/sys/imp/stacks.rs`).
use mozjs::jsapi::{
    BuildStackString, CaptureCurrentStack, GetSavedFrameColumn, GetSavedFrameFunctionDisplayName,
    GetSavedFrameLine, GetSavedFrameParent, GetSavedFrameSource, JSObject, JSString,
    JS_GetStringLength, JS_NewContext, JS_StackCapture_MaxFrames, SavedFrameResult,
    SavedFrameSelfHosted, JS,
};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr;

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

#[test]
fn capture_is_an_empty_stack() {
    unsafe {
        let cx = JS_NewContext(32 << 20, ptr::null_mut());
        let mut capture = MaybeUninit::uninit();
        JS_StackCapture_MaxFrames(128, capture.as_mut_ptr());
        let mut capture = capture.assume_init();
        let mut stack: *mut JSObject = 1 as *mut JSObject;
        let none: *mut JSObject = ptr::null_mut();
        assert!(CaptureCurrentStack(
            cx,
            handle_mut(&mut stack),
            &mut capture,
            handle(&none)
        ));
        assert!(stack.is_null());

        let sh = SavedFrameSelfHosted::Include;
        let mut s: *mut JSString = ptr::null_mut();
        let r = GetSavedFrameSource(cx, ptr::null_mut(), handle(&stack), handle_mut(&mut s), sh);
        assert_eq!(r, SavedFrameResult::AccessDenied);
        assert!(!s.is_null() && JS_GetStringLength(s) == 0);
        let mut name: *mut JSString = 1 as *mut JSString;
        let r = GetSavedFrameFunctionDisplayName(
            cx,
            ptr::null_mut(),
            handle(&stack),
            handle_mut(&mut name),
            sh,
        );
        assert_eq!(r, SavedFrameResult::AccessDenied);
        assert!(name.is_null());
        let mut line = 9;
        assert_eq!(
            GetSavedFrameLine(cx, ptr::null_mut(), handle(&stack), &mut line, sh),
            SavedFrameResult::AccessDenied
        );
        assert_eq!(line, 0);
        let mut column = JS::TaggedColumnNumberOneOrigin { value_: 9 };
        GetSavedFrameColumn(cx, ptr::null_mut(), handle(&stack), &mut column, sh);
        assert_eq!(column.value_, 0);
        let mut parent: *mut JSObject = 1 as *mut JSObject;
        let r = GetSavedFrameParent(
            cx,
            ptr::null_mut(),
            handle(&stack),
            handle_mut(&mut parent),
            sh,
        );
        assert_eq!(r, SavedFrameResult::AccessDenied);
        assert!(parent.is_null());

        let mut text: *mut JSString = ptr::null_mut();
        assert!(BuildStackString(
            cx,
            ptr::null_mut(),
            handle(&stack),
            handle_mut(&mut text),
            0,
            mozjs::jsapi::js::StackFormat::Default,
        ));
        assert_eq!(JS_GetStringLength(text), 0);
    }
}

#[test]
fn scripted_caller_is_the_running_script() {
    use mozjs::glue::DescribeScriptedCaller;
    unsafe {
        let cx = JS_NewContext(32 << 20, ptr::null_mut());
        let mut buf = [1 as std::ffi::c_char; 16];
        let (mut line, mut col) = (5, 5);
        // No script is running.
        assert!(!DescribeScriptedCaller(
            cx,
            buf.as_mut_ptr(),
            buf.len(),
            &mut line,
            &mut col
        ));
        assert_eq!((line, col), (0, 0));
    }
}
