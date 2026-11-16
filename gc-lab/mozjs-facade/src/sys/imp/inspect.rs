/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Object inspection and JSON, as Servo's console previews its arguments
//! (components/script/dom/console.rs `console_object_from_handle_value`,
//! `console_map_object_from_handle_value`, `stringify_handle_value`) and its
//! DOM bindings read JSON (MOZJS_FACADE.md "Inspection and JSON").
//!
//! - `GetBuiltinClass` maps the engine's [`BuiltinClass`] (its internal
//!   slots) onto `js::ESClass`.
//! - `MapSize`/`MapEntries` read a Map's [[MapData]]; `GetArrayLength` is
//!   LengthOfArrayLike (ECMA-262 §7.3.19) checked against u32.
//! - `JS_Stringify` and `JS_ParseJSON*` run the engine's JSON.stringify and
//!   JSON.parse (§25.5).
//! - `JS_GetFunctionId`/`JS_GetFunctionDisplayId` read the function's own
//!   "name" data property and `JS_GetFunctionArity` its own "length": the
//!   engine keeps no separate explicit/inferred atom or `nargs`, so both ids
//!   answer the SetFunctionName name (null when empty).
//! - `JS_ValueToSource` follows SpiderMonkey's `ValueToSource` for
//!   primitives; objects print as array/object literals (two levels, cycles
//!   as `{}`/`[]`) and functions as their source text. It is a diagnostic
//!   string only (console fallback), not an evaluable round trip.

use std::os::raw::c_void;
use std::ptr;

use gc_lab::script::{BuiltinClass, Engine, JsValue, ObjectId, PropertyKey, ScriptError};

use super::context::{context, Context};
use super::natives::{from_jsval, handle_object, handle_object_or_null, string_cell, to_jsval};
use super::objects::engine_result;
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;

unsafe fn handle_value(v: JS::Handle<JS::Value>) -> JsValue {
    // SAFETY: a Handle points to a rooted, live location.
    unsafe { from_jsval(*v.ptr) }
}

fn write_value(cx: &Context, out: *mut JS::Value, value: Option<JsValue>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let v = cx.heap.with_engine(|engine| to_jsval(cx, engine, &value));
    // SAFETY: the caller's MutableHandle location.
    unsafe { *out = v };
    true
}

fn es_class(class: BuiltinClass) -> root::js::ESClass {
    use root::js::ESClass as E;
    match class {
        BuiltinClass::Object => E::Object,
        BuiltinClass::Array => E::Array,
        BuiltinClass::Number => E::Number,
        BuiltinClass::String => E::String,
        BuiltinClass::Boolean => E::Boolean,
        BuiltinClass::RegExp => E::RegExp,
        BuiltinClass::ArrayBuffer => E::ArrayBuffer,
        BuiltinClass::SharedArrayBuffer => E::SharedArrayBuffer,
        BuiltinClass::Date => E::Date,
        BuiltinClass::Set => E::Set,
        BuiltinClass::Map => E::Map,
        BuiltinClass::Promise => E::Promise,
        BuiltinClass::MapIterator => E::MapIterator,
        BuiltinClass::SetIterator => E::SetIterator,
        BuiltinClass::Arguments => E::Arguments,
        BuiltinClass::Error => E::Error,
        BuiltinClass::BigInt => E::BigInt,
        BuiltinClass::Function => E::Function,
        BuiltinClass::Other => E::Other,
    }
}

// --- Builtin class and collections ---

/// `JS::GetBuiltinClass`: never fails for a facade object.
pub unsafe fn GetBuiltinClass(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    cls: *mut root::js::ESClass,
) -> bool {
    let cx = unsafe { context(cx) };
    let object = unsafe { handle_object(obj) };
    let class = cx.heap.with_engine(|engine| engine.builtin_class(object));
    // SAFETY: the caller's out-parameter.
    unsafe { *cls = es_class(class) };
    true
}

/// `JS::MapSize`: SpiderMonkey asserts a Map; a non-Map answers 0.
pub unsafe fn MapSize(cx: *mut root::JSContext, obj: JS::HandleObject) -> u32 {
    let cx = unsafe { context(cx) };
    let object = unsafe { handle_object(obj) };
    let size = cx.heap.with_engine(|engine| engine.map_size(object));
    size.map_or(0, |n| n.min(u32::MAX as usize) as u32)
}

/// `JS::MapEntries`: a Map iterator over [key, value] pairs.
pub unsafe fn MapEntries(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    rval: JS::MutableHandleValue,
) -> bool {
    let cx = unsafe { context(cx) };
    let object = unsafe { handle_object(obj) };
    let entries = cx.heap.with_engine(|engine| engine.map_entries(object));
    write_value(cx, rval.ptr, engine_result(cx, entries))
}

/// `JS::GetArrayLength`: LengthOfArrayLike, a RangeError above u32 as
/// SpiderMonkey's `GetLengthProperty` reports.
pub unsafe fn GetArrayLength(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    lengthp: *mut u32,
) -> bool {
    let cx = unsafe { context(cx) };
    let object = unsafe { handle_object(obj) };
    let length = cx.heap.with_engine(|engine| {
        let n = engine.length_of_array_like(object)?;
        u32::try_from(n).map_err(|_| engine.throw_new_error("RangeError", "invalid array length"))
    });
    let Some(length) = engine_result(cx, length) else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *lengthp = length };
    true
}

// --- Functions ---

fn own_data(engine: &mut Engine<'_>, object: ObjectId, key: &str) -> Option<JsValue> {
    let property = engine.own_property(object, PropertyKey::String(key.into()));
    property.ok().flatten().and_then(|d| d.value)
}

fn function_name(cx: &Context, fun: *mut root::JSFunction, out: *mut *mut root::JSString) -> bool {
    let object = unsafe { super::objects::object_cell(fun as *mut root::JSObject) }.id;
    let name = cx
        .heap
        .with_engine(|engine| own_data(engine, object, "name"));
    let name = name.and_then(|v| v.as_string()).filter(|s| !s.is_empty());
    let string = name.map_or(ptr::null_mut(), |s| cx.heap.atoms.string(s));
    // SAFETY: the caller's MutableHandle location.
    unsafe { *out = string };
    true
}

/// `JS_GetFunctionId`: the function's name, null when anonymous.
pub unsafe fn JS_GetFunctionId(
    cx: *mut root::JSContext,
    fun: JS::Handle<*mut root::JSFunction>,
    name: JS::MutableHandle<*mut root::JSString>,
) -> bool {
    let cx = unsafe { context(cx) };
    // SAFETY: a Handle points to a rooted, live location.
    function_name(cx, unsafe { *fun.ptr }, name.ptr)
}

/// `JS_GetFunctionDisplayId`: the same name (module docs).
pub unsafe fn JS_GetFunctionDisplayId(
    cx: *mut root::JSContext,
    fun: JS::Handle<*mut root::JSFunction>,
    name: JS::MutableHandle<*mut root::JSString>,
) -> bool {
    let cx = unsafe { context(cx) };
    // SAFETY: a Handle points to a rooted, live location.
    function_name(cx, unsafe { *fun.ptr }, name.ptr)
}

/// `JS_GetFunctionArity`: the own "length" data property (module docs).
pub unsafe fn JS_GetFunctionArity(fun: *mut root::JSFunction) -> u16 {
    let cell = unsafe { super::objects::object_cell(fun as *mut root::JSObject) };
    let cx = cell.context();
    let length = cx
        .heap
        .with_engine(|engine| own_data(engine, cell.id, "length"));
    length
        .and_then(|v| v.as_number())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map_or(0, |n| n.min(u16::MAX as f64) as u16)
}

/// `JS_ValueToFunction`: the function object, or a TypeError.
pub unsafe fn JS_ValueToFunction(
    cx: *mut root::JSContext,
    v: JS::HandleValue,
) -> *mut root::JSFunction {
    let cx = unsafe { context(cx) };
    // SAFETY: a Handle points to a rooted, live location.
    let raw = unsafe { *v.ptr };
    let value = unsafe { handle_value(v) };
    let callable = cx.heap.with_engine(|engine| {
        if engine.is_callable(&value) {
            Ok(())
        } else {
            let shown = value_to_source(engine, &value, 1);
            Err(engine.throw_new_error("TypeError", &format!("{shown} is not a function")))
        }
    });
    match engine_result(cx, callable) {
        Some(()) => raw.to_object() as *mut root::JSFunction,
        None => ptr::null_mut(),
    }
}

// --- Source and JSON ---

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// SpiderMonkey's `ValueToSource` (js/src/builtin/Object.cpp) shape.
fn value_to_source(engine: &mut Engine<'_>, value: &JsValue, depth: usize) -> String {
    if value.is_undefined() {
        return "(void 0)".into();
    }
    if value.is_null() {
        return "null".into();
    }
    if let Some(b) = value.as_bool() {
        return b.to_string();
    }
    if let Some(n) = value.as_number() {
        if n == 0.0 && n.is_sign_negative() {
            return "-0".into();
        }
        return engine.to_string_value(value).unwrap_or_default();
    }
    if let Some(s) = value.as_string() {
        return quote(&s);
    }
    if let Some(id) = value.as_symbol() {
        let text = engine.symbol_descriptive_string(id);
        let description = text
            .strip_prefix("Symbol(")
            .and_then(|t| t.strip_suffix(')'))
            .unwrap_or("");
        return if description.is_empty() {
            "Symbol()".into()
        } else {
            format!("Symbol({})", quote(description))
        };
    }
    let Some(object) = value.as_object() else {
        return String::new();
    };
    if engine.is_callable(value) {
        return engine
            .to_string_value(value)
            .unwrap_or_else(|_| "function () {}".into());
    }
    let array = engine.builtin_class(object) == BuiltinClass::Array;
    if depth == 0 {
        return if array { "[]" } else { "({})" }.into();
    }
    let Ok(keys) = engine.own_property_keys(object) else {
        return "({})".into();
    };
    let mut parts = Vec::new();
    for key in keys {
        let PropertyKey::String(name) = key else {
            continue;
        };
        if array && name == "length" {
            continue;
        }
        let Ok(Some(descriptor)) = engine.own_property(object, PropertyKey::String(name.clone()))
        else {
            continue;
        };
        if !descriptor.enumerable {
            continue;
        }
        let Some(v) = descriptor.value else {
            continue;
        };
        let shown = value_to_source(engine, &v, depth - 1);
        parts.push(if array {
            shown
        } else {
            format!("{}:{shown}", quote(&name))
        });
    }
    if array {
        format!("[{}]", parts.join(", "))
    } else {
        format!("({{{}}})", parts.join(", "))
    }
}

/// `JS_ValueToSource` (module docs).
pub unsafe fn JS_ValueToSource(
    cx: *mut root::JSContext,
    v: JS::Handle<JS::Value>,
) -> *mut root::JSString {
    let cx = unsafe { context(cx) };
    let value = unsafe { handle_value(v) };
    let text = cx
        .heap
        .with_engine(|engine| value_to_source(engine, &value, 2));
    cx.heap.atoms.string(text)
}

/// `JS_Stringify`: JSON.stringify, the text handed to `callback` in one
/// UTF-16 chunk; nothing is written when the result is undefined.
pub unsafe fn JS_Stringify(
    cx: *mut root::JSContext,
    value: JS::MutableHandle<JS::Value>,
    replacer: JS::Handle<*mut root::JSObject>,
    space: JS::Handle<JS::Value>,
    callback: root::JSONWriteCallback,
    data: *mut c_void,
) -> bool {
    let cx = unsafe { context(cx) };
    // SAFETY: a MutableHandle points to a rooted, live location.
    let input = unsafe { from_jsval(*value.ptr) };
    let replacer = unsafe { handle_object_or_null(replacer) };
    let space = unsafe { handle_value(space) };
    let text = cx
        .heap
        .with_engine(|engine| engine.json_stringify(&input, replacer, &space));
    let Some(text) = engine_result(cx, text) else {
        return false;
    };
    let (Some(text), Some(callback)) = (text, callback) else {
        return true;
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let Ok(len) = u32::try_from(units.len()) else {
        return false;
    };
    // SAFETY: the embedder's callback, given `len` units it may only read.
    unsafe { callback(units.as_ptr(), len, data) }
}

fn parse_json(cx: &Context, text: Result<String, ScriptError>, vp: *mut JS::Value) -> bool {
    let parsed = cx.heap.with_engine(|engine| engine.json_parse(&text?));
    write_value(cx, vp, engine_result(cx, parsed))
}

fn utf16_text(units: &[u16]) -> Result<String, ScriptError> {
    String::from_utf16(units)
        .map_err(|_| ScriptError::Unsupported("JSON unpaired UTF-16 surrogate"))
}

/// `JS_ParseJSON` over UTF-16 code units.
pub unsafe fn JS_ParseJSON(
    cx: *mut root::JSContext,
    chars: *const u16,
    len: u32,
    vp: JS::MutableHandle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    let units: &[u16] = if len == 0 {
        &[]
    } else {
        // SAFETY: the caller's `len` UTF-16 units.
        unsafe { std::slice::from_raw_parts(chars, len as usize) }
    };
    parse_json(cx, utf16_text(units), vp.ptr)
}

/// `JS_ParseJSON` over a JSString.
pub unsafe fn JS_ParseJSON1(
    cx: *mut root::JSContext,
    str_: JS::Handle<*mut root::JSString>,
    vp: JS::MutableHandle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    // SAFETY: a Handle points to a rooted, live location.
    let text = unsafe { string_cell(*str_.ptr) }.text.clone();
    parse_json(cx, Ok(text), vp.ptr)
}

/// `JS_ParseJSON` over Latin-1 characters.
pub unsafe fn JS_ParseJSON2(
    cx: *mut root::JSContext,
    chars: *const root::JS::Latin1Char,
    len: u32,
    vp: JS::MutableHandle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    let bytes: &[u8] = if len == 0 {
        &[]
    } else {
        // SAFETY: the caller's `len` Latin-1 characters.
        unsafe { std::slice::from_raw_parts(chars, len as usize) }
    };
    let text = bytes.iter().map(|&b| b as char).collect();
    parse_json(cx, Ok(text), vp.ptr)
}
