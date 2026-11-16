/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Classic scripts: compile options, `JS::Compile`, `JS_ExecuteScript`,
//! `JS::Evaluate`, script privates, the hidden scripted caller and the error
//! report Servo builds from an uncaught exception
//! (components/script/dom/globalscope.rs `evaluate_js_on_global`,
//! components/script/dom/bindings/error.rs `ErrorInfo::from_native_error`).
//!
//! A `JSScript*` is a leaked [`ScriptCell`] holding the source text. Compile
//! runs the parser for its early errors (ECMA-262 §16.1.6 ParseScript), and
//! execution runs ScriptEvaluation (§16.1.6) on the same text, so the text is
//! parsed twice; the engine has no separate compiled-script object yet.

use std::cell::Cell;
use std::collections::hash_map::Entry;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_uint, c_void};
use std::ptr;

use gc_lab::script::{JsValue, ObjectId, ScriptError};

use super::context::{context, Context};
use super::natives::{from_jsval, handle_object, to_jsval};
use super::objects::engine_result;
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;
use crate::sys::jsval::{JSVal, UndefinedValue};

// --- Compile options (glue: NewCompileOptions / DeleteCompileOptions) ---

/// `JS::OwningCompileOptions` stand-in: the options first, so the pointer
/// handed out is a `ReadOnlyCompileOptions*`, then the filename it points to.
#[repr(C)]
struct OwnedCompileOptions {
    options: JS::ReadOnlyCompileOptions,
    filename: CString,
}

unsafe fn c_string(text: *const c_char) -> CString {
    if text.is_null() {
        CString::default()
    } else {
        // SAFETY: the caller passes a NUL-terminated string.
        unsafe { CStr::from_ptr(text) }.to_owned()
    }
}

pub unsafe fn NewCompileOptions(
    _cx: *mut root::JSContext,
    file: *const c_char,
    line: c_uint,
) -> *mut JS::ReadOnlyCompileOptions {
    let mut owned = Box::new(OwnedCompileOptions {
        // SAFETY: a plain C struct whose all-zero value is SpiderMonkey's
        // default (null strings, false flags, enum discriminants 0).
        options: unsafe { std::mem::zeroed() },
        filename: unsafe { c_string(file) },
    });
    owned.options._base.filename_.data_ = owned.filename.as_ptr();
    owned.options.lineno = line;
    owned.options.column._base = 1;
    Box::into_raw(owned) as *mut JS::ReadOnlyCompileOptions
}

pub unsafe fn DeleteCompileOptions(opts: *mut JS::ReadOnlyCompileOptions) {
    if !opts.is_null() {
        // SAFETY: only NewCompileOptions makes these, as a Box.
        drop(unsafe { Box::from_raw(opts as *mut OwnedCompileOptions) });
    }
}

// --- Scripts ---

/// The object behind a `JSScript*`. Never freed, like every cell.
pub struct ScriptCell {
    /// The context that compiled it; its runtime holds the private hooks.
    context: *const Context,
    source: String,
    pub filename: String,
    pub line: u32,
    no_script_rval: bool,
    private: Cell<JSVal>,
}

unsafe fn script_cell<'a>(script: *mut root::JSScript) -> &'a ScriptCell {
    assert!(!script.is_null(), "mozjs facade: null JSScript");
    // SAFETY: every JSScript* comes from `compile` below.
    unsafe { &*(script as *const ScriptCell) }
}

/// The source of a UTF-8 SourceText. Malformed UTF-8 is a SyntaxError, as in
/// SpiderMonkey's tokenizer.
unsafe fn utf8_source(
    engine: &mut gc_lab::script::Engine<'static>,
    src: *mut JS::SourceText<root::mozilla::Utf8Unit>,
) -> Result<String, ScriptError> {
    // SAFETY: a live SourceText with `length_` units at `units_`.
    let src = unsafe { &*src };
    if src.units_.is_null() || src.length_ == 0 {
        return Ok(String::new());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(src.units_ as *const u8, src.length_ as usize) };
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(_) => Err(engine.throw_new_error("SyntaxError", "malformed UTF-8 character sequence")),
    }
}

/// The source of a UTF-16 SourceText. The engine takes source as `str`, so a
/// lone surrogate (valid only inside string literals and comments) cannot be
/// represented; that source is refused as unsupported, never altered.
unsafe fn utf16_source(src: *mut JS::SourceText<u16>) -> Result<String, ScriptError> {
    // SAFETY: as in `utf8_source`.
    let src = unsafe { &*src };
    if src.units_.is_null() || src.length_ == 0 {
        return Ok(String::new());
    }
    let units = unsafe { std::slice::from_raw_parts(src.units_, src.length_ as usize) };
    String::from_utf16(units)
        .map_err(|_| ScriptError::Unsupported("script source with a lone surrogate".into()))
}

unsafe fn compile(
    cx: *mut root::JSContext,
    options: *const JS::ReadOnlyCompileOptions,
    source: impl FnOnce(&mut gc_lab::script::Engine<'static>) -> Result<String, ScriptError>,
) -> *mut root::JSScript {
    let context = unsafe { context(cx) };
    let checked = context.heap.with_engine(|engine| {
        let source = source(engine)?;
        engine.check_script(&source).map(|()| source)
    });
    let Some(source) = engine_result(context, checked) else {
        return ptr::null_mut();
    };
    // SAFETY: Servo passes options from NewCompileOptions (or its own
    // CompileOptions, same layout).
    let options = unsafe { &*options };
    let filename = unsafe { c_string(options._base.filename_.data_) };
    let cell = Box::new(ScriptCell {
        context: context as *const Context,
        source,
        filename: filename.to_string_lossy().into_owned(),
        line: options.lineno,
        no_script_rval: options.noScriptRval,
        private: Cell::new(UndefinedValue()),
    });
    Box::leak(cell) as *mut ScriptCell as *mut root::JSScript
}

pub unsafe fn Compile(
    cx: *mut root::JSContext,
    options: *const JS::ReadOnlyCompileOptions,
    src: *mut JS::SourceText<u16>,
) -> *mut root::JSScript {
    unsafe { compile(cx, options, |_| utf16_source(src)) }
}

pub unsafe fn Compile1(
    cx: *mut root::JSContext,
    options: *const JS::ReadOnlyCompileOptions,
    src: *mut JS::SourceText<root::mozilla::Utf8Unit>,
) -> *mut root::JSScript {
    unsafe { compile(cx, options, |engine| utf8_source(engine, src)) }
}

/// ScriptEvaluation (§16.1.6) of `source` in the current realm; the
/// completion value goes to `rval` unless the options said noScriptRval.
fn evaluate(
    cx: &Context,
    filename: String,
    source: Result<String, ScriptError>,
    rval: Option<JS::MutableHandle<JS::Value>>,
) -> bool {
    cx.state.borrow_mut().running_scripts.push(filename);
    let result = cx.heap.with_engine(|engine| {
        let value = engine.evaluate_script(&source?)?;
        Ok(to_jsval(cx, engine, &value))
    });
    cx.state.borrow_mut().running_scripts.pop();
    match engine_result(cx, result) {
        Some(value) => {
            if let Some(rval) = rval {
                // SAFETY: a live MutableHandle from the caller.
                unsafe { *rval.ptr = value };
            }
            true
        }
        None => false,
    }
}

pub unsafe fn JS_ExecuteScript(
    cx: *mut root::JSContext,
    script: JS::Handle<*mut root::JSScript>,
    rval: JS::MutableHandle<JS::Value>,
) -> bool {
    let script = unsafe { script_cell(*script.ptr) };
    let context = unsafe { context(cx) };
    if script.no_script_rval {
        unsafe { *rval.ptr = UndefinedValue() };
        return evaluate(
            context,
            script.filename.clone(),
            Ok(script.source.clone()),
            None,
        );
    }
    evaluate(
        context,
        script.filename.clone(),
        Ok(script.source.clone()),
        Some(rval),
    )
}

pub unsafe fn JS_ExecuteScript1(
    cx: *mut root::JSContext,
    script: JS::Handle<*mut root::JSScript>,
) -> bool {
    let script = unsafe { script_cell(*script.ptr) };
    evaluate(
        unsafe { context(cx) },
        script.filename.clone(),
        Ok(script.source.clone()),
        None,
    )
}

pub unsafe fn Evaluate(
    cx: *mut root::JSContext,
    options: *const JS::ReadOnlyCompileOptions,
    src: *mut JS::SourceText<u16>,
    rval: JS::MutableHandle<JS::Value>,
) -> bool {
    evaluate(
        unsafe { context(cx) },
        unsafe { options_filename(options) },
        unsafe { utf16_source(src) },
        Some(rval),
    )
}

pub unsafe fn Evaluate2(
    cx: *mut root::JSContext,
    options: *const JS::ReadOnlyCompileOptions,
    src: *mut JS::SourceText<root::mozilla::Utf8Unit>,
    rval: JS::MutableHandle<JS::Value>,
) -> bool {
    let context = unsafe { context(cx) };
    let source = context
        .heap
        .with_engine(|engine| unsafe { utf8_source(engine, src) });
    let filename = unsafe { options_filename(options) };
    evaluate(context, filename, source, Some(rval))
}

/// The filename compile options name (empty when they name none).
unsafe fn options_filename(options: *const JS::ReadOnlyCompileOptions) -> String {
    if options.is_null() {
        return String::new();
    }
    // SAFETY: Servo passes options from NewCompileOptions (or its own
    // CompileOptions, same layout).
    let filename = unsafe { (*options)._base.filename_.data_ };
    unsafe { c_string(filename) }.to_string_lossy().into_owned()
}

// --- Script privates ---

/// Set the embedder's private value, telling the runtime's reference hooks
/// (SetScriptPrivateReferenceHooks) about the new and the old value, as
/// SpiderMonkey's ScriptSourceObject::setPrivate does.
pub unsafe fn SetScriptPrivate(script: *mut root::JSScript, value: *const JS::Value) {
    let script = unsafe { script_cell(script) };
    let new = unsafe { *value };
    let old = script.private.replace(new);
    // SAFETY: contexts outlive their scripts (JS_DestroyContext comes last).
    let hooks = unsafe { &*script.context }
        .state
        .borrow()
        .script_private_hooks;
    if let Some((add_ref, release)) = hooks {
        if !new.is_undefined() {
            if let Some(add_ref) = add_ref {
                unsafe { add_ref(&new) };
            }
        }
        if !old.is_undefined() {
            if let Some(release) = release {
                unsafe { release(&old) };
            }
        }
    }
}

pub unsafe fn JS_GetScriptPrivate(script: *mut root::JSScript, dest: JS::MutableHandleValue) {
    let script = unsafe { script_cell(script) };
    unsafe { *dest.ptr = script.private.get() };
}

// --- Scripted caller ---

/// Only nesting is tracked; DescribeScriptedCaller reads it (the facade has
/// no GetScriptedCallerGlobal yet).
pub unsafe fn HideScriptedCaller(cx: *mut root::JSContext) {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .hidden_scripted_caller += 1;
}

pub unsafe fn UnhideScriptedCaller(cx: *mut root::JSContext) {
    let context = unsafe { context(cx) };
    let mut state = context.state.borrow_mut();
    state.hidden_scripted_caller = state
        .hidden_scripted_caller
        .checked_sub(1)
        .expect("mozjs facade: UnhideScriptedCaller without HideScriptedCaller");
}

/// jsglue.cpp `DescribeScriptedCaller` over JS::DescribeScriptedCaller
/// (js/src/jsapi.cpp): false when no script is running or the caller is
/// hidden; otherwise the running script's filename, NUL-terminated and cut to
/// `buflen`. The engine does not track the current position yet, so line and
/// column are 0, the "unknown" values (MOZJS_FACADE.md "Stack capture").
/// Only scripts entered through the facade's evaluate count: a function a
/// script defined, called later by the embedder, reports no caller.
pub unsafe fn DescribeScriptedCaller(
    cx: *mut root::JSContext,
    buffer: *mut c_char,
    buflen: usize,
    line: *mut u32,
    col: *mut u32,
) -> bool {
    unsafe {
        *line = 0;
        *col = 0;
    }
    if buflen == 0 {
        return false;
    }
    let state = unsafe { context(cx) }.state.borrow();
    if state.hidden_scripted_caller > 0 {
        return false;
    }
    let Some(filename) = state.running_scripts.last() else {
        return false;
    };
    let bytes = filename.as_bytes();
    let n = bytes.len().min(buflen - 1);
    // SAFETY: the caller's buffer holds `buflen` bytes.
    let out = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), buflen) };
    out[..n].copy_from_slice(&bytes[..n]);
    out[n] = 0;
    true
}

// --- Error reports ---

/// A JSErrorReport and the strings it points to.
pub struct ErrorReportCell {
    report: root::JSErrorReport,
    _message: CString,
    _filename: CString,
}

fn exception_type(class: &str) -> root::JSExnType {
    use root::JSExnType::*;
    match class {
        "InternalError" => JSEXN_INTERNALERR,
        "AggregateError" => JSEXN_AGGREGATEERR,
        "EvalError" => JSEXN_EVALERR,
        "RangeError" => JSEXN_RANGEERR,
        "ReferenceError" => JSEXN_REFERENCEERR,
        "SuppressedError" => JSEXN_SUPPRESSEDERR,
        "SyntaxError" => JSEXN_SYNTAXERR,
        "TypeError" => JSEXN_TYPEERR,
        "URIError" => JSEXN_URIERR,
        _ => JSEXN_ERR,
    }
}

/// The error report of an object with an [[ErrorData]] slot, or false for
/// any other object (SpiderMonkey's ErrorFromException returns null there).
///
/// The message is the error's own `message` data property as created, as in
/// ErrorObject::getOrCreateErrorReport (no "Name: " prefix). The engine does
/// not record where an error was created yet, so the filename is empty, the
/// line 0 and the column 1 ("unknown" in SpiderMonkey's terms).
pub unsafe fn JS_ErrorFromException(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    error_report: *mut JS::BorrowedErrorReport,
) -> bool {
    let context = unsafe { context(cx) };
    let id = unsafe { handle_object(obj) };
    let Some(report) = cached_error_report(context, id) else {
        return false;
    };
    // SAFETY: the caller's live BorrowedErrorReport.
    unsafe {
        (*error_report).owner_.data = *obj.ptr;
        (*error_report).report_ = report;
    }
    true
}

/// The cached report of an [[ErrorData]] object, or None for any other object.
fn cached_error_report(context: &Context, id: ObjectId) -> Option<*mut root::JSErrorReport> {
    let (class, message) = context.heap.with_engine(|engine| {
        let class = engine.error_class(id)?;
        let message = match engine.own_property(id, "message") {
            Ok(Some(d)) => d.value.as_ref().and_then(JsValue::as_string),
            _ => None,
        };
        Some((class, message.unwrap_or_default()))
    })?;
    let mut state = context.state.borrow_mut();
    let cell = match state.error_reports.entry(id) {
        Entry::Occupied(e) => e.into_mut(),
        Entry::Vacant(e) => {
            let message = CString::new(message.replace('\0', "\u{FFFD}")).unwrap_or_default();
            let filename = CString::default();
            // SAFETY: a plain C struct; all-zero is an empty report.
            let mut report: root::JSErrorReport = unsafe { std::mem::zeroed() };
            report._base.message_.data_ = message.as_ptr();
            report._base.filename.data_ = filename.as_ptr();
            report._base.lineno = 0;
            report._base.column._base = 1;
            report.exnType = exception_type(&class) as i16;
            e.insert(Box::new(ErrorReportCell {
                report,
                _message: message,
                _filename: filename,
            }))
        }
    };
    Some(&mut cell.report as *mut root::JSErrorReport)
}

/// Glue `PendingExceptionStackInfo` (jsglue.cpp): steals the pending
/// exception and describes it as `JS::ErrorReportBuilder::init`
/// (js/src/vm/ErrorObject.cpp) does with side effects allowed:
///
/// - an [[ErrorData]] object gives its report (`JS_ErrorFromException`);
/// - any other value gives the JSMSG_UNCAUGHT_EXCEPTION report
///   "uncaught exception: " + ToString(value), where a Symbol uses
///   SymbolDescriptiveString (ECMA-262 §20.4.3.3.1) and a ToString that
///   throws gives "unknown (can't convert to string)" with the exception
///   dropped. That report's position comes from the saved stack frame,
///   which the engine does not record, so it has no filename (the callback
///   is not called for it), line 0 and column 1.
///
/// The message and filename go to `callback`; `dest` gets the exception.
/// Returns false when no exception is pending.
pub unsafe fn PendingExceptionStackInfo(
    cx: *mut root::JSContext,
    callback: crate::glue::StringCallback,
    message_target: *mut c_void,
    filename_target: *mut c_void,
    line: *mut u32,
    col: *mut u32,
    dest: JS::MutableHandleValue,
) -> bool {
    let context = unsafe { context(cx) };
    let Some(exception) = context.state.borrow_mut().pending_exception.take() else {
        return false;
    };
    let value = unsafe { from_jsval(exception) };
    let report = value
        .as_object()
        .and_then(|id| cached_error_report(context, id));
    let (message, filename, lineno, column) = match report {
        Some(report) => {
            // SAFETY: a boxed report owned by the context's state, whose
            // strings live as long as it does.
            let report = unsafe { &*report };
            let base = &report._base;
            let message = unsafe { CStr::from_ptr(base.message_.data_) }.to_owned();
            let filename = unsafe { CStr::from_ptr(base.filename.data_) }.to_owned();
            (message, Some(filename), base.lineno, base.column._base)
        }
        None => {
            let text = context.heap.with_engine(|engine| match value.as_symbol() {
                Some(symbol) => Ok(engine.symbol_descriptive_string(symbol)),
                None => engine.to_string_value(&value),
            });
            // A throwing ToString leaves its exception pending in
            // SpiderMonkey's init until clearPendingException; here the
            // engine's error is simply dropped.
            let text = text.unwrap_or_else(|_| "unknown (can't convert to string)".to_owned());
            (c_text(&format!("uncaught exception: {text}")), None, 0, 1)
        }
    };
    if let Some(callback) = callback {
        let bytes = message.as_bytes();
        // SAFETY: the glue's StringCallback contract: a pointer and length.
        unsafe { callback(bytes.as_ptr().cast(), bytes.len(), message_target) };
        if let Some(filename) = filename {
            let bytes = filename.as_bytes();
            unsafe { callback(bytes.as_ptr().cast(), bytes.len(), filename_target) };
        }
    }
    // SAFETY: the caller's out-parameters.
    unsafe {
        *line = lineno;
        *col = column;
        *dest.ptr = exception;
    }
    true
}

/// UTF-8 text as a C string, cut at the first NUL as `strlen` would.
fn c_text(text: &str) -> CString {
    let end = text.find('\0').unwrap_or(text.len());
    CString::new(&text[..end]).unwrap_or_default()
}
