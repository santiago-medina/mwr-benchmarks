/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! JSNative functions, strings, symbols and property definition: what
//! Servo's WebIDL interface setup (`script_bindings/interface.rs`,
//! `constructor.rs`) runs on every new global (MOZJS_FACADE.md "Natives,
//! strings and interface objects").
//!
//! JSNative dispatch. A host function's word indexes [`Shared::natives`];
//! `Host::call` lands in [`call_native`], which lays out SpiderMonkey's `vp`
//! array (`[callee, this | JS_IS_CONSTRUCTING magic, args..., new.target]`,
//! js/public/CallArgs.h) and calls the native. The native reenters the facade
//! through entry points that need the engine, but the engine is the caller's
//! `&mut Engine` at that point. So the running engine is detached to
//! `'static` and parked in `Heap::engine` for the duration of the call, and a
//! hostless spare engine takes its place behind the caller's reference; after
//! the call the two swap back. A native that returns false without a pending
//! exception is an uncatchable error, as in SpiderMonkey.
//!
//! Strings are interned, immutable [`StringCell`]s holding both the Latin-1
//! (when it fits) and UTF-16 forms Servo reads; symbols are [`SymbolCell`]s.
//! Both follow the C1 cell policy of objects.rs: never freed.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_uint};
use std::ptr;
use std::rc::Rc;

use gc_lab::script::{Engine, JsValue, ObjectId, PropertyKey, ScriptError};

use super::context::{context, Context};
use super::objects::{engine_result, new_object, object_cell, Shared};
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;
use crate::sys::jsid::SymbolId;
use crate::sys::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, MagicValue, NullValue, ObjectValue, StringValue,
    SymbolValue, UndefinedValue,
};

/// A host function: the native and its JIT info (Servo's `JSJitInfo` drives
/// its generic method, getter and setter trampolines).
#[derive(Clone)]
pub struct Native {
    pub(super) op: root::JSNative,
    info: *const root::JSJitInfo,
    /// The function object; there is exactly one per native word.
    object: Cell<Option<ObjectId>>,
}

/// A `JSString*`.
#[repr(C, align(8))]
pub struct StringCell {
    pub(super) text: String,
    latin1: Option<Box<[u8]>>,
    pub(super) utf16: Box<[u16]>,
}

/// A `JS::Symbol*`: the engine's symbol number.
#[repr(C, align(8))]
pub struct SymbolCell {
    pub(super) id: u64,
}

/// Interned string and symbol cells of a context.
#[derive(Default)]
pub struct Atoms {
    strings: RefCell<HashMap<String, &'static StringCell>>,
    symbols: RefCell<HashMap<u64, &'static SymbolCell>>,
}

impl Atoms {
    pub(super) fn string(&self, text: String) -> *mut root::JSString {
        let mut strings = self.strings.borrow_mut();
        let cell = *strings.entry(text.clone()).or_insert_with(|| {
            let utf16: Box<[u16]> = text.encode_utf16().collect();
            let latin1 = utf16
                .iter()
                .all(|&u| u <= 0xff)
                .then(|| utf16.iter().map(|&u| u as u8).collect());
            Box::leak(Box::new(StringCell {
                text,
                latin1,
                utf16,
            }))
        });
        cell as *const StringCell as *mut root::JSString
    }

    pub(super) fn symbol(&self, id: u64) -> *mut JS::Symbol {
        let cell = *self
            .symbols
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| Box::leak(Box::new(SymbolCell { id })));
        cell as *const SymbolCell as *mut JS::Symbol
    }
}

pub(super) unsafe fn string_cell<'a>(s: *mut root::JSString) -> &'a StringCell {
    assert!(!s.is_null(), "mozjs facade: null JSString");
    // SAFETY: JSString pointers come from StringCell leaks only.
    unsafe { &*(s as *const StringCell) }
}

pub(super) unsafe fn symbol_cell<'a>(s: *mut JS::Symbol) -> &'a SymbolCell {
    assert!(!s.is_null(), "mozjs facade: null Symbol");
    // SAFETY: Symbol pointers come from SymbolCell leaks only.
    unsafe { &*(s as *const SymbolCell) }
}

// --- Value conversion ---

/// The engine value of a `JS::Value`. Needs no engine: strings, symbols and
/// objects carry their engine identity in their cells.
pub(super) unsafe fn from_jsval(v: JSVal) -> JsValue {
    if v.is_undefined() {
        JsValue::undefined()
    } else if v.is_null() {
        JsValue::null()
    } else if v.is_boolean() {
        JsValue::boolean(v.to_boolean())
    } else if v.is_number() {
        JsValue::number(v.to_number())
    } else if v.is_string() {
        JsValue::string(&unsafe { string_cell(v.to_string()) }.text)
    } else if v.is_symbol() {
        JsValue::symbol(unsafe { symbol_cell(v.to_symbol()) }.id)
    } else if v.is_object() {
        JsValue::object(unsafe { object_cell(v.to_object()) }.id)
    } else {
        panic!("mozjs facade: BigInt and magic values are not supported yet")
    }
}

/// The `JS::Value` of an engine value: integral numbers that fit are Int32,
/// as SpiderMonkey canonicalizes them (`JS::NumberValue`).
pub(super) fn to_jsval(cx: &Context, engine: &Engine<'_>, v: &JsValue) -> JSVal {
    let heap = &cx.heap;
    if v.is_undefined() {
        UndefinedValue()
    } else if v.is_null() {
        NullValue()
    } else if let Some(b) = v.as_bool() {
        BooleanValue(b)
    } else if let Some(n) = v.as_number() {
        let i = n as i32;
        if i as f64 == n && !(n == 0.0 && n.is_sign_negative()) {
            Int32Value(i)
        } else {
            DoubleValue(n)
        }
    } else if let Some(s) = v.as_string() {
        // SAFETY: a fresh or interned StringCell.
        StringValue(unsafe { &*heap.atoms.string(s) })
    } else if let Some(id) = v.as_symbol() {
        // SAFETY: a fresh or interned SymbolCell.
        SymbolValue(unsafe { &*heap.atoms.symbol(id) })
    } else if let Some(id) = v.as_object() {
        ObjectValue(heap.object(cx, engine, id, ptr::null()))
    } else {
        panic!("mozjs facade: BigInt values are not supported yet")
    }
}

// --- JSNative dispatch ---

pub(super) fn call_native(
    shared: &Rc<Shared>,
    engine: &mut Engine<'_>,
    function: u64,
    this: JsValue,
    args: &[JsValue],
    new_target: Option<ObjectId>,
) -> Result<JsValue, ScriptError> {
    let cx_ptr = shared.context.get() as *mut root::JSContext;
    // SAFETY: set by JS_NewContext; the context outlives its engine.
    let cx = unsafe { context(cx_ptr) };
    let heap = &cx.heap;
    let native = shared.natives.borrow()[function as usize].clone();
    let op = native
        .op
        .expect("mozjs facade: host function without a JSNative");
    let callee = native
        .object
        .get()
        .expect("mozjs facade: JSNative called before its function object exists");

    // CallArgs layout (js/public/CallArgs.h).
    let mut vp = Vec::with_capacity(args.len() + 3);
    vp.push(ObjectValue(heap.object(cx, engine, callee, ptr::null())));
    vp.push(match new_target {
        Some(_) => MagicValue(root::JSWhyMagic::JS_IS_CONSTRUCTING as u32),
        None => to_jsval(cx, engine, &this),
    });
    vp.extend(args.iter().map(|a| to_jsval(cx, engine, a)));
    if let Some(target) = new_target {
        vp.push(ObjectValue(heap.object(cx, engine, target, ptr::null())));
    }

    // Park the running engine; a spare stands in behind `engine`.
    let spare = match heap.take_spare().detach() {
        Ok(spare) => spare,
        Err(_) => unreachable!("spare engines have no adapter"),
    };
    let running = std::mem::replace(engine, spare);
    let running = match running.detach() {
        Ok(running) => running,
        Err(running) => {
            let spare = std::mem::replace(engine, running);
            heap.spares.borrow_mut().extend(spare.detach().ok());
            return Err(ScriptError::Runtime(
                "mozjs facade: JSNative call on an engine with a DOM adapter",
            ));
        }
    };
    {
        let mut slot = heap.engine.borrow_mut();
        assert!(slot.is_none(), "mozjs facade: engine parked twice");
        *slot = Some(running);
    }

    // SAFETY: the embedder's JSNative, called with SpiderMonkey's contract.
    let ok = unsafe { op(cx_ptr, args.len() as c_uint, vp.as_mut_ptr()) };

    let running = heap
        .engine
        .borrow_mut()
        .take()
        .expect("mozjs facade: parked engine lost during a JSNative");
    let running = match running.detach() {
        Ok(running) => running,
        Err(_) => unreachable!("the parked engine had no adapter"),
    };
    let spare = std::mem::replace(engine, running);
    heap.spares.borrow_mut().extend(spare.detach().ok());

    if !ok {
        // A JSNative throws by leaving a pending exception and returning
        // false; false with none pending is an uncatchable error (OOM-like).
        let pending = cx.state.borrow_mut().pending_exception.take();
        return Err(match pending {
            // SAFETY: a JSVal the embedder stored with JS_SetPendingException.
            Some(value) => engine.throw_value(unsafe { from_jsval(value) }),
            None => {
                ScriptError::Runtime("mozjs facade: JSNative failed without a pending exception")
            }
        });
    }
    // SAFETY: vp[0] holds the return value (CallArgs::rval).
    Ok(unsafe { from_jsval(vp[0]) })
}

/// A function object dispatching to `op`.
fn new_native(
    cx: &Context,
    engine: &mut Engine<'_>,
    name: &str,
    nargs: usize,
    op: root::JSNative,
    info: *const root::JSJitInfo,
    constructor: bool,
) -> Result<ObjectId, ScriptError> {
    let shared = &cx.heap.shared;
    let word = {
        let mut natives = shared.natives.borrow_mut();
        natives.push(Native {
            op,
            info,
            object: Cell::new(None),
        });
        natives.len() - 1
    };
    let id = engine.create_function(name, nargs, word as u64, constructor)?;
    shared.natives.borrow()[word].object.set(Some(id));
    shared.native_of.borrow_mut().insert(id, word);
    Ok(id)
}

// --- Helpers ---

/// A C string name, read as Latin-1 (SpiderMonkey atomizes `const char*`
/// names that way).
pub(super) unsafe fn latin1(name: *const c_char) -> String {
    assert!(!name.is_null(), "mozjs facade: null name");
    // SAFETY: a NUL-terminated string from the embedder.
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    bytes.iter().map(|&b| b as char).collect()
}

pub(super) unsafe fn handle_object(h: JS::Handle<*mut root::JSObject>) -> ObjectId {
    // SAFETY: a Handle points to a rooted, live location.
    unsafe { object_cell(*h.ptr) }.id
}

pub(super) unsafe fn handle_object_or_null(h: JS::Handle<*mut root::JSObject>) -> Option<ObjectId> {
    // SAFETY: a Handle points to a rooted, live location.
    let obj = unsafe { *h.ptr };
    (!obj.is_null()).then(|| unsafe { object_cell(obj) }.id)
}

/// The property key of a `jsid` (integer ids become their decimal string,
/// ECMA-262 §7.1.17 CanonicalNumericIndexString's inverse).
pub(super) unsafe fn key_of(id: root::jsid) -> PropertyKey {
    if id.is_int() {
        PropertyKey::String(id.to_int().to_string())
    } else if id.is_string() {
        PropertyKey::String(unsafe { string_cell(id.to_string()) }.text.clone())
    } else if id.is_symbol() {
        PropertyKey::Symbol(unsafe { symbol_cell(id.to_symbol()) }.id)
    } else {
        panic!("mozjs facade: void jsid used as a property key")
    }
}

/// (writable, enumerable, configurable) of JSPROP_* attributes.
pub(super) fn attributes(attrs: c_uint) -> (bool, bool, bool) {
    (
        attrs & root::JSPROP_READONLY as c_uint == 0,
        attrs & root::JSPROP_ENUMERATE as c_uint != 0,
        attrs & root::JSPROP_PERMANENT as c_uint == 0,
    )
}

/// JS::SymbolCode order (js/public/Symbol.h), by description.
const SYMBOL_CODES: [&str; 15] = [
    "isConcatSpreadable",
    "iterator",
    "match",
    "replace",
    "search",
    "species",
    "hasInstance",
    "split",
    "toPrimitive",
    "toStringTag",
    "unscopables",
    "asyncIterator",
    "matchAll",
    "dispose",
    "asyncDispose",
];

fn well_known(engine: &Engine<'_>, code: usize) -> u64 {
    let name = SYMBOL_CODES
        .get(code)
        .unwrap_or_else(|| panic!("mozjs facade: unknown JS::SymbolCode {code}"));
    engine
        .well_known_symbol(name)
        .unwrap_or_else(|| panic!("mozjs facade: no well-known symbol @@{name}"))
}

/// The key and function name of a spec name (JSPropertySpec::Name): a C
/// string, or a well-known symbol encoded as `SymbolCode + 1`. None ends the
/// spec list.
unsafe fn spec_name(
    engine: &Engine<'_>,
    name: &root::JSPropertySpec_Name,
) -> Option<(PropertyKey, String)> {
    // SAFETY: both members are pointer-sized; the bits decide which is meant.
    let bits = unsafe { name.symbol_ };
    if bits == 0 {
        None
    } else if bits <= SYMBOL_CODES.len() {
        let code = bits - 1;
        Some((
            PropertyKey::Symbol(well_known(engine, code)),
            format!("[Symbol.{}]", SYMBOL_CODES[code]),
        ))
    } else {
        let text = unsafe { latin1(name.string_) };
        Some((PropertyKey::String(text.clone()), text))
    }
}

/// The intrinsic a self-hosted JSFunctionSpec names. Servo uses only the
/// Array iteration methods (iterable and maplike interfaces).
fn self_hosted(engine: &mut Engine<'_>, name: &str) -> Result<Option<JsValue>, ScriptError> {
    let method = match name {
        "$ArrayValues" | "ArrayValues" => "values",
        "ArrayKeys" => "keys",
        "ArrayEntries" => "entries",
        "ArrayForEach" => "forEach",
        _ => return Ok(None),
    };
    let Some(array) = engine.array_prototype() else {
        return Ok(None);
    };
    engine.get_property(array, method).map(Some)
}

fn define_value(
    cx: &Context,
    object: ObjectId,
    key: PropertyKey,
    value: JsValue,
    attrs: c_uint,
) -> bool {
    let (w, e, c) = attributes(attrs);
    let defined = cx
        .heap
        .with_engine(|engine| engine.define_property(object, key, value, w, e, c));
    engine_result(cx, defined).unwrap_or(false)
}

unsafe fn define_jsval(
    cx: &Context,
    object: ObjectId,
    key: PropertyKey,
    value: JSVal,
    attrs: c_uint,
) -> bool {
    define_value(cx, object, key, unsafe { from_jsval(value) }, attrs)
}

/// An accessor whose getter and setter are JSNatives, named "get x" / "set x"
/// (ECMA-262 §10.2.9 SetFunctionName with a prefix).
pub(super) fn define_native_accessor(
    cx: &Context,
    object: ObjectId,
    key: PropertyKey,
    name: &str,
    getter: root::JSNativeWrapper,
    setter: root::JSNativeWrapper,
    attrs: c_uint,
) -> bool {
    let (_, e, c) = attributes(attrs);
    let defined = cx.heap.with_engine(|engine| {
        let getter = match getter.op {
            Some(_) => Some(new_native(
                cx,
                engine,
                &format!("get {name}"),
                0,
                getter.op,
                getter.info,
                false,
            )?),
            None => None,
        };
        // The getter is reachable from nothing until define_accessor: root
        // it while the setter allocates (a collection there would free it).
        let held = match getter {
            Some(id) => Some(engine.root(id)?),
            None => None,
        };
        let setter = match setter.op {
            Some(_) => new_native(
                cx,
                engine,
                &format!("set {name}"),
                1,
                setter.op,
                setter.info,
                false,
            )
            .map(Some),
            None => Ok(None),
        };
        if let Some(root) = held {
            engine.unroot(root)?;
        }
        engine.define_accessor(object, key, getter, setter?, e, c)
    });
    engine_result(cx, defined).unwrap_or(false)
}

fn define_object_accessor(
    cx: &Context,
    object: ObjectId,
    key: PropertyKey,
    getter: Option<ObjectId>,
    setter: Option<ObjectId>,
    attrs: c_uint,
) -> bool {
    let (_, e, c) = attributes(attrs);
    let defined = cx
        .heap
        .with_engine(|engine| engine.define_accessor(object, key, getter, setter, e, c));
    engine_result(cx, defined).unwrap_or(false)
}

pub(super) fn native_wrapper(op: root::JSNative) -> root::JSNativeWrapper {
    root::JSNativeWrapper {
        op,
        info: ptr::null(),
    }
}

pub(super) fn object_ptr(cx: &Context, id: Option<ObjectId>) -> *mut root::JSObject {
    match id {
        Some(id) => cx
            .heap
            .with_engine(|engine| cx.heap.object(cx, engine, id, ptr::null())),
        None => ptr::null_mut(),
    }
}

// --- Associated memory (js/public/MemoryFunctions.h) ---

/// Only accounted: the collector's heap limit does not include external
/// memory yet.
pub unsafe fn AddAssociatedMemory(obj: *mut root::JSObject, nbytes: usize, _use: JS::MemoryUse) {
    let bytes = &unsafe { object_cell(obj) }.context().heap.associated_bytes;
    bytes.set(bytes.get() + nbytes);
}

pub unsafe fn RemoveAssociatedMemory(obj: *mut root::JSObject, nbytes: usize, _use: JS::MemoryUse) {
    let bytes = &unsafe { object_cell(obj) }.context().heap.associated_bytes;
    bytes.set(bytes.get().saturating_sub(nbytes));
}

// --- Strings (js/public/String.h) ---

pub unsafe fn JS_AtomizeAndPinString(
    cx: *mut root::JSContext,
    s: *const c_char,
) -> *mut root::JSString {
    let text = unsafe { latin1(s) };
    unsafe { context(cx) }.heap.atoms.string(text)
}

pub unsafe fn JS_NewStringCopyN(
    cx: *mut root::JSContext,
    s: *const c_char,
    n: usize,
) -> *mut root::JSString {
    let bytes: &[u8] = if n == 0 {
        &[]
    } else {
        // SAFETY: the caller passes `n` readable bytes.
        unsafe { std::slice::from_raw_parts(s as *const u8, n) }
    };
    let text = bytes.iter().map(|&b| b as char).collect();
    unsafe { context(cx) }.heap.atoms.string(text)
}

pub unsafe fn JS_NewStringCopyZ(cx: *mut root::JSContext, s: *const c_char) -> *mut root::JSString {
    if s.is_null() {
        return unsafe { context(cx) }.heap.atoms.string(String::new());
    }
    let text = unsafe { latin1(s) };
    unsafe { context(cx) }.heap.atoms.string(text)
}

/// Invalid UTF-8 fails (SpiderMonkey reports a malformed-UTF-8 error).
pub unsafe fn JS_NewStringCopyUTF8N(
    cx: *mut root::JSContext,
    s: *const JS::UTF8Chars,
) -> *mut root::JSString {
    // SAFETY: a live UTF8Chars range.
    let range = unsafe { &(*s)._base };
    let (start, end) = (range.mStart.mPtr, range.mEnd.mPtr);
    let len = end as usize - start as usize;
    let bytes: &[u8] = if len == 0 {
        &[]
    } else {
        // SAFETY: [start, end) is the caller's buffer.
        unsafe { std::slice::from_raw_parts(start as *const u8, len) }
    };
    match std::str::from_utf8(bytes) {
        Ok(text) => unsafe { context(cx) }.heap.atoms.string(text.into()),
        Err(_) => ptr::null_mut(),
    }
}

pub unsafe fn JS_GetLatin1StringCharsAndLength(
    _cx: *mut root::JSContext,
    _nogc: *const JS::AutoRequireNoGC,
    str_: *mut root::JSString,
    length: *mut usize,
) -> *const JS::Latin1Char {
    let cell = unsafe { string_cell(str_) };
    let chars = cell
        .latin1
        .as_ref()
        .expect("mozjs facade: Latin-1 chars of a two-byte string");
    // SAFETY: the caller's out-parameter.
    unsafe { *length = chars.len() };
    chars.as_ptr()
}

pub unsafe fn JS_GetTwoByteStringCharsAndLength(
    _cx: *mut root::JSContext,
    _nogc: *const JS::AutoRequireNoGC,
    str_: *mut root::JSString,
    length: *mut usize,
) -> *const u16 {
    let cell = unsafe { string_cell(str_) };
    // SAFETY: the caller's out-parameter.
    unsafe { *length = cell.utf16.len() };
    cell.utf16.as_ptr()
}

pub unsafe fn JS_DeprecatedStringHasLatin1Chars(str_: *mut root::JSString) -> bool {
    unsafe { string_cell(str_) }.latin1.is_some()
}

pub unsafe fn JS_GetStringLength(str_: *mut root::JSString) -> usize {
    unsafe { string_cell(str_) }.utf16.len()
}

// --- Functions (jsapi.h, jsfriendapi.h) ---

/// A JSFunction* is the function object's cell (there is no separate
/// function representation).
pub unsafe fn JS_NewFunction(
    cx: *mut root::JSContext,
    call: root::JSNative,
    nargs: c_uint,
    flags: c_uint,
    name: *const c_char,
) -> *mut root::JSFunction {
    let cx = unsafe { context(cx) };
    let name = if name.is_null() {
        String::new()
    } else {
        unsafe { latin1(name) }
    };
    let constructor = flags & root::JSFUN_CONSTRUCTOR != 0;
    let created = cx.heap.with_engine(|engine| {
        new_native(
            cx,
            engine,
            &name,
            nargs as usize,
            call,
            ptr::null(),
            constructor,
        )
    });
    let Some(id) = engine_result(cx, created) else {
        return ptr::null_mut();
    };
    object_ptr(cx, Some(id)) as *mut root::JSFunction
}

pub unsafe fn JS_GetFunctionObject(fun: *mut root::JSFunction) -> *mut root::JSObject {
    fun as *mut root::JSObject
}

/// The JIT info of a native function value; null for anything else.
pub unsafe fn RUST_FUNCTION_VALUE_TO_JITINFO(v: JS::Value) -> *const root::JSJitInfo {
    if !v.is_object() {
        return ptr::null();
    }
    let cell = unsafe { object_cell(v.to_object()) };
    let shared = &cell.context().heap.shared;
    let word = shared.native_of.borrow().get(&cell.id).copied();
    word.map_or(ptr::null(), |w| shared.natives.borrow()[w].info)
}

// --- Property definition (jsapi.h) ---

pub unsafe fn JS_DefineProperty(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    value: JS::Handle<JS::Value>,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(unsafe { latin1(name) });
    unsafe { define_jsval(context(cx), handle_object(obj), key, *value.ptr, attrs) }
}

pub unsafe fn JS_DefineProperty1(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    getter: root::JSNative,
    setter: root::JSNative,
    attrs: c_uint,
) -> bool {
    let name = unsafe { latin1(name) };
    let key = PropertyKey::String(name.clone());
    unsafe {
        define_native_accessor(
            context(cx),
            handle_object(obj),
            key,
            &name,
            native_wrapper(getter),
            native_wrapper(setter),
            attrs,
        )
    }
}

pub unsafe fn JS_DefineProperty2(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    getter: JS::Handle<*mut root::JSObject>,
    setter: JS::Handle<*mut root::JSObject>,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(unsafe { latin1(name) });
    unsafe {
        define_object_accessor(
            context(cx),
            handle_object(obj),
            key,
            handle_object_or_null(getter),
            handle_object_or_null(setter),
            attrs,
        )
    }
}

pub unsafe fn JS_DefineProperty3(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    value: JS::Handle<*mut root::JSObject>,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(unsafe { latin1(name) });
    let value = JsValue::object(unsafe { handle_object(value) });
    unsafe { define_value(context(cx), handle_object(obj), key, value, attrs) }
}

pub unsafe fn JS_DefineProperty4(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    value: JS::Handle<*mut root::JSString>,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(unsafe { latin1(name) });
    let value = JsValue::string(&unsafe { string_cell(*value.ptr) }.text);
    unsafe { define_value(context(cx), handle_object(obj), key, value, attrs) }
}

pub unsafe fn JS_DefineProperty5(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    value: i32,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(unsafe { latin1(name) });
    let value = JsValue::number(value as f64);
    unsafe { define_value(context(cx), handle_object(obj), key, value, attrs) }
}

pub unsafe fn JS_DefineProperty6(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    value: u32,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(unsafe { latin1(name) });
    let value = JsValue::number(value as f64);
    unsafe { define_value(context(cx), handle_object(obj), key, value, attrs) }
}

pub unsafe fn JS_DefineProperty7(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    value: f64,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(unsafe { latin1(name) });
    let value = JsValue::number(value);
    unsafe { define_value(context(cx), handle_object(obj), key, value, attrs) }
}

pub unsafe fn JS_DefinePropertyById2(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    id: JS::Handle<root::jsid>,
    value: JS::Handle<JS::Value>,
    attrs: c_uint,
) -> bool {
    unsafe {
        let key = key_of(*id.ptr);
        define_jsval(context(cx), handle_object(obj), key, *value.ptr, attrs)
    }
}

pub unsafe fn JS_DefinePropertyById4(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    id: JS::Handle<root::jsid>,
    getter: JS::Handle<*mut root::JSObject>,
    setter: JS::Handle<*mut root::JSObject>,
    attrs: c_uint,
) -> bool {
    unsafe {
        let key = key_of(*id.ptr);
        define_object_accessor(
            context(cx),
            handle_object(obj),
            key,
            handle_object_or_null(getter),
            handle_object_or_null(setter),
            attrs,
        )
    }
}

pub unsafe fn JS_DefinePropertyById5(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    id: JS::Handle<root::jsid>,
    value: JS::Handle<*mut root::JSObject>,
    attrs: c_uint,
) -> bool {
    unsafe {
        let key = key_of(*id.ptr);
        let value = JsValue::object(handle_object(value));
        define_value(context(cx), handle_object(obj), key, value, attrs)
    }
}

pub unsafe fn JS_DefineElement(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    index: u32,
    value: JS::Handle<JS::Value>,
    attrs: c_uint,
) -> bool {
    let key = PropertyKey::String(index.to_string());
    unsafe { define_jsval(context(cx), handle_object(obj), key, *value.ptr, attrs) }
}

/// JSFunctionSpec lists (JS_FS / JS_SELF_HOSTED_FN). `flags` carries the
/// property attributes and JSFUN_CONSTRUCTOR.
pub unsafe fn JS_DefineFunctions(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    fs: *const root::JSFunctionSpec,
) -> bool {
    let cx = unsafe { context(cx) };
    let object = unsafe { handle_object(obj) };
    let mut spec = fs;
    loop {
        // SAFETY: a spec list ends with a zero name.
        let entry = unsafe { &*spec };
        let named = cx
            .heap
            .with_engine(|engine| unsafe { spec_name(engine, &entry.name) });
        let Some((key, name)) = named else {
            return true;
        };
        let flags = entry.flags as c_uint;
        let value = cx.heap.with_engine(|engine| {
            if entry.selfHostedName.is_null() {
                let constructor = flags & root::JSFUN_CONSTRUCTOR != 0;
                new_native(
                    cx,
                    engine,
                    &name,
                    entry.nargs as usize,
                    entry.call.op,
                    entry.call.info,
                    constructor,
                )
                .map(|id| Some(JsValue::object(id)))
            } else {
                self_hosted(engine, &unsafe { latin1(entry.selfHostedName) })
            }
        });
        let Some(value) = engine_result(cx, value).flatten() else {
            eprintln!("mozjs facade: cannot define function {name}");
            return false;
        };
        if !define_value(cx, object, key, value, flags) {
            return false;
        }
        spec = unsafe { spec.add(1) };
    }
}

/// JSPropertySpec lists (JS_PSG / JS_PSGS / JS_STRING_PS / JS_INT32_PS ...).
pub unsafe fn JS_DefineProperties(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    ps: *const root::JSPropertySpec,
) -> bool {
    let cx = unsafe { context(cx) };
    let object = unsafe { handle_object(obj) };
    let mut spec = ps;
    loop {
        // SAFETY: a spec list ends with a zero name.
        let entry = unsafe { &*spec };
        let named = cx
            .heap
            .with_engine(|engine| unsafe { spec_name(engine, &entry.name) });
        let Some((key, name)) = named else {
            return true;
        };
        let attrs = entry.attributes_ as c_uint;
        let ok = match entry.kind_ {
            root::JSPropertySpec_Kind::NativeAccessor => {
                // SAFETY: `accessors.*.native` is the active member for
                // NativeAccessor.
                let (getter, setter) = unsafe {
                    (
                        entry.u.accessors.getter.native,
                        entry.u.accessors.setter.native,
                    )
                };
                define_native_accessor(cx, object, key, &name, getter, setter, attrs)
            }
            root::JSPropertySpec_Kind::Value => {
                // SAFETY: `value` is the active member for Value; its type
                // tag selects the inner member.
                let value = unsafe {
                    let wrapper = &entry.u.value;
                    match wrapper.type_ {
                        root::JSPropertySpec_ValueWrapper_Type::String => {
                            JsValue::string(&latin1(wrapper.__bindgen_anon_1.string))
                        }
                        root::JSPropertySpec_ValueWrapper_Type::Int32 => {
                            JsValue::number(wrapper.__bindgen_anon_1.int32 as f64)
                        }
                        root::JSPropertySpec_ValueWrapper_Type::Double => {
                            JsValue::number(wrapper.__bindgen_anon_1.double_)
                        }
                    }
                };
                define_value(cx, object, key, value, attrs)
            }
            root::JSPropertySpec_Kind::SelfHostedAccessor => {
                eprintln!("mozjs facade: self-hosted accessor {name} is not supported");
                false
            }
        };
        if !ok {
            return false;
        }
        spec = unsafe { spec.add(1) };
    }
}

// --- Prototypes ---

/// `ctor.prototype` (non-writable, non-enumerable, non-configurable) and
/// `proto.constructor` (writable, configurable), as ECMA-262 §10.2.5
/// MakeConstructor and §20.2.3.? lay them out.
pub unsafe fn JS_LinkConstructorAndPrototype(
    cx: *mut root::JSContext,
    ctor: JS::Handle<*mut root::JSObject>,
    proto: JS::Handle<*mut root::JSObject>,
) -> bool {
    let cx = unsafe { context(cx) };
    let (ctor, proto) = unsafe { (handle_object(ctor), handle_object(proto)) };
    let linked = cx.heap.with_engine(|engine| {
        Ok::<_, ScriptError>(
            engine.define_property(
                ctor,
                "prototype",
                JsValue::object(proto),
                false,
                false,
                false,
            )? && engine.define_property(
                proto,
                "constructor",
                JsValue::object(ctor),
                true,
                false,
                true,
            )?,
        )
    });
    engine_result(cx, linked).unwrap_or(false)
}

/// False when [[SetPrototypeOf]] refuses (SpiderMonkey also reports a
/// TypeError; pending exceptions come with E5).
pub unsafe fn JS_SetPrototype(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    proto: JS::HandleObject,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, proto) = unsafe { (handle_object(obj), handle_object_or_null(proto)) };
    let set = cx
        .heap
        .with_engine(|engine| engine.set_object_prototype(obj, proto));
    engine_result(cx, set).unwrap_or(false)
}

/// Immutable prototype exotic object (ECMA-262 §10.4.7), used by WebIDL for
/// global prototype chains (WebIDL §3.7.1).
pub unsafe fn JS_SetImmutablePrototype(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    succeeded: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let obj = unsafe { handle_object(obj) };
    let set = cx
        .heap
        .with_engine(|engine| engine.set_immutable_prototype(obj));
    let Some(set) = engine_result(cx, set) else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *succeeded = set };
    true
}

pub unsafe fn JS_GetPrototype(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    result: JS::MutableHandleObject,
) -> bool {
    let cx = unsafe { context(cx) };
    let obj = unsafe { handle_object(obj) };
    let proto = cx.heap.with_engine(|engine| engine.prototype_of(obj));
    let Some(proto) = engine_result(cx, proto) else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *result.ptr = object_ptr(cx, proto) };
    true
}

// --- Property access ---

pub unsafe fn JS_GetProperty(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    vp: JS::MutableHandleValue,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, name) = unsafe { (handle_object(obj), latin1(name)) };
    let value = cx.heap.with_engine(|engine| {
        engine
            .get_property(obj, name.as_str())
            .map(|v| to_jsval(cx, engine, &v))
    });
    let Some(value) = engine_result(cx, value) else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *vp.ptr = value };
    true
}

pub unsafe fn JS_HasProperty(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    foundp: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, name) = unsafe { (handle_object(obj), latin1(name)) };
    let found = cx
        .heap
        .with_engine(|engine| engine.has_property(obj, name.as_str()));
    let Some(found) = engine_result(cx, found) else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *foundp = found };
    true
}

// --- Symbols ---

pub unsafe fn GetWellKnownSymbol(
    cx: *mut root::JSContext,
    which: JS::SymbolCode,
) -> *mut JS::Symbol {
    let cx = unsafe { context(cx) };
    let id = cx
        .heap
        .with_engine(|engine| well_known(engine, which as usize));
    cx.heap.atoms.symbol(id)
}

pub unsafe fn RUST_SYMBOL_TO_JSID(sym: *mut JS::Symbol, id: JS::MutableHandleId) {
    // SAFETY: the caller's out-parameter.
    unsafe { *id.ptr = SymbolId(sym) };
}

// --- Realm intrinsics (js/public/Realm.h) ---

fn realm_intrinsic(
    cx: *mut root::JSContext,
    pick: impl FnOnce(&Engine<'static>) -> Option<ObjectId>,
) -> *mut root::JSObject {
    let cx = unsafe { context(cx) };
    let id = cx.heap.with_engine(|engine| pick(engine));
    object_ptr(cx, id)
}

pub unsafe fn GetRealmObjectPrototype(cx: *mut root::JSContext) -> *mut root::JSObject {
    realm_intrinsic(cx, |e| e.object_prototype())
}

pub unsafe fn GetRealmFunctionPrototype(cx: *mut root::JSContext) -> *mut root::JSObject {
    realm_intrinsic(cx, |e| e.function_prototype())
}

pub unsafe fn GetRealmErrorPrototype(cx: *mut root::JSContext) -> *mut root::JSObject {
    realm_intrinsic(cx, |e| e.error_prototype())
}

pub unsafe fn GetRealmIteratorPrototype(cx: *mut root::JSContext) -> *mut root::JSObject {
    realm_intrinsic(cx, |e| e.iterator_prototype())
}

pub unsafe fn JS_NewPlainObject(cx: *mut root::JSContext) -> *mut root::JSObject {
    let proto = unsafe { context(cx) }
        .heap
        .with_engine(|engine| engine.object_prototype());
    unsafe { new_object(cx, ptr::null(), proto) }
}

// --- Wrappers and the debugger ---
//
// The engine has no cross-compartment wrappers (objects.rs): every object is
// its own unwrapped self and wrapping into a compartment is the identity.

pub unsafe fn UncheckedUnwrapObject(
    obj: *mut root::JSObject,
    _stop_at_window_proxy: bool,
) -> *mut root::JSObject {
    obj
}

pub unsafe fn CheckedUnwrapStatic(obj: *mut root::JSObject) -> *mut root::JSObject {
    obj
}

pub unsafe fn IsWrapper(_obj: *mut root::JSObject) -> bool {
    false
}

pub unsafe fn JS_WrapObject(_cx: *mut root::JSContext, _objp: JS::MutableHandleObject) -> bool {
    true
}

pub unsafe fn JS_WrapValue(_cx: *mut root::JSContext, _vp: JS::MutableHandleValue) -> bool {
    true
}

/// There is no Debugger API: Servo's devtools debugger global gets no
/// `Debugger` constructor, and its scripts fail where they use it.
pub unsafe fn JS_DefineDebuggerObject(cx: *mut root::JSContext, _obj: JS::HandleObject) -> bool {
    unsafe { context(cx) };
    true
}
