/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Property access by `jsid`, property descriptors, id vectors, linear
//! strings, calls and the small predicates Servo's generated bindings and
//! proxy handlers run on every DOM object (MOZJS_FACADE.md "Property access
//! and calls").
//!
//! Integer ids. SpiderMonkey stores array indices up to `JSID_INT_MAX`
//! (2^31 - 1) as int jsids and every other key as an atom; [`id_of`] keeps
//! that canonical form, so an id Servo compares by bits (`jsid == jsid`)
//! matches the one the engine hands back. [`key_of`] maps both forms to the
//! same engine key.
//!
//! An `IdVector` (`JS::PersistentRootedIdVector`) is an [`IdVectorCell`]:
//! the ids hold only interned strings and symbols, which C1 never frees, so
//! the vector needs no tracing.

use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_uint, c_void};
use std::ptr;

use gc_lab::script::{
    Engine, JsValue, ObjectId, PartialDescriptor, PropertyDescriptor, PropertyKey,
};

use super::context::{context, Context, GetWindowProxyClass};
use super::natives::{
    attributes, define_native_accessor, from_jsval, handle_object, handle_object_or_null, key_of,
    latin1, native_wrapper, object_ptr, string_cell, to_jsval,
};
use super::objects::{engine_result, object_cell};
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;
use crate::sys::jsid::{IntId, StringId, SymbolId};
use crate::sys::jsval::{Int32Value, JSVal, StringValue, SymbolValue, UndefinedValue};

// --- Helpers ---

/// The array index a string denotes (ECMA-262 §6.1.7: a canonical numeric
/// string of an integer below 2^32 - 1).
fn array_index(units: impl Iterator<Item = u16> + Clone) -> Option<u32> {
    let mut count = 0usize;
    let mut value: u64 = 0;
    let mut first = 0u16;
    for unit in units {
        if !(b'0' as u16..=b'9' as u16).contains(&unit) {
            return None;
        }
        if count == 0 {
            first = unit;
        }
        count += 1;
        value = value * 10 + (unit - b'0' as u16) as u64;
        if value >= u32::MAX as u64 {
            return None;
        }
    }
    if count == 0 || (count > 1 && first == b'0' as u16) {
        return None;
    }
    Some(value as u32)
}

/// The canonical jsid of a property key (see the module comment).
fn id_of(cx: &Context, key: &PropertyKey) -> root::jsid {
    match key {
        PropertyKey::String(text) => match array_index(text.encode_utf16()) {
            Some(i) if i <= i32::MAX as u32 => IntId(i as i32),
            _ => StringId(cx.heap.atoms.string(text.clone())),
        },
        PropertyKey::Symbol(id) => SymbolId(cx.heap.atoms.symbol(*id)),
    }
}

unsafe fn handle_jsval(v: JS::Handle<JS::Value>) -> JsValue {
    // SAFETY: a Handle points to a rooted, live location.
    unsafe { from_jsval(*v.ptr) }
}

/// The arguments of a `JS::HandleValueArray`.
unsafe fn value_array(args: *const JS::HandleValueArray) -> Vec<JsValue> {
    if args.is_null() {
        return Vec::new();
    }
    // SAFETY: the caller's array of `length_` rooted values.
    let args = unsafe { &*args };
    if args.length_ == 0 {
        return Vec::new();
    }
    let values = unsafe { std::slice::from_raw_parts(args.elements_, args.length_) };
    values.iter().map(|&v| unsafe { from_jsval(v) }).collect()
}

/// Writes `value` (converted in the engine) to an out-parameter.
fn write_value(cx: &Context, out: *mut JS::Value, value: Option<JsValue>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let value = cx.heap.with_engine(|engine| to_jsval(cx, engine, &value));
    // SAFETY: the caller's out-parameter.
    unsafe { *out = value };
    true
}

fn write_flag(out: *mut bool, flag: Option<bool>) -> bool {
    let Some(flag) = flag else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *out = flag };
    true
}

/// `ObjectOpResult::succeed` or `fail(code)`.
unsafe fn op_result(
    result: *mut JS::ObjectOpResult,
    ok: Option<bool>,
    code: root::JSErrNum,
) -> bool {
    let Some(ok) = ok else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe {
        (*result).code_ = if ok {
            JS::ObjectOpResult_SpecialCodes::OkCode as usize
        } else {
            code as usize
        };
    }
    true
}

/// Throws a TypeError (an ObjectOpResult failure reported in strict mode,
/// ObjectOpResult::checkStrict).
fn throw_type_error(cx: &Context, message: &str) -> bool {
    let error = cx
        .heap
        .with_engine(|engine| Err::<(), _>(engine.throw_new_error("TypeError", message)));
    engine_result(cx, error);
    false
}

fn key_text(key: &PropertyKey) -> String {
    match key {
        PropertyKey::String(text) => text.clone(),
        PropertyKey::Symbol(_) => String::new(),
    }
}

fn blank_descriptor() -> JS::PropertyDescriptor {
    JS::PropertyDescriptor {
        _bitfield_align_1: [],
        _bitfield_1: root::__BindgenBitfieldUnit::new([0; 2]),
        getter_: ptr::null_mut(),
        setter_: ptr::null_mut(),
        value_: UndefinedValue(),
    }
}

/// A complete `JS::PropertyDescriptor` of an engine descriptor.
fn to_descriptor(
    cx: &Context,
    engine: &Engine<'_>,
    d: &PropertyDescriptor,
) -> JS::PropertyDescriptor {
    let mut out = blank_descriptor();
    out.set_hasConfigurable_(true);
    out.set_configurable_(d.configurable);
    out.set_hasEnumerable_(true);
    out.set_enumerable_(d.enumerable);
    match &d.value {
        Some(value) => {
            out.set_hasWritable_(true);
            out.set_writable_(d.writable);
            out.set_hasValue_(true);
            out.value_ = to_jsval(cx, engine, value);
        }
        None => {
            out.set_hasGetter_(true);
            out.set_hasSetter_(true);
            out.getter_ = d.getter.map_or(ptr::null_mut(), |g| {
                cx.heap.object(cx, engine, g, ptr::null())
            });
            out.setter_ = d.setter.map_or(ptr::null_mut(), |s| {
                cx.heap.object(cx, engine, s, ptr::null())
            });
        }
    }
    out
}

/// The partial descriptor a `JS::PropertyDescriptor` holds (§6.2.6).
unsafe fn from_descriptor(d: &JS::PropertyDescriptor) -> PartialDescriptor {
    let object = |o: *mut root::JSObject| (!o.is_null()).then(|| unsafe { object_cell(o) }.id);
    PartialDescriptor {
        value: d.hasValue_().then(|| unsafe { from_jsval(d.value_) }),
        writable: d.hasWritable_().then(|| d.writable_()),
        get: d.hasGetter_().then(|| object(d.getter_)),
        set: d.hasSetter_().then(|| object(d.setter_)),
        enumerable: d.hasEnumerable_().then(|| d.enumerable_()),
        configurable: d.hasConfigurable_().then(|| d.configurable_()),
    }
}

fn define(cx: &Context, obj: ObjectId, key: PropertyKey, desc: PartialDescriptor) -> Option<bool> {
    let defined = cx
        .heap
        .with_engine(|engine| engine.define_own_property(obj, key, desc));
    engine_result(cx, defined)
}

fn get(cx: &Context, obj: ObjectId, key: PropertyKey, out: *mut JS::Value) -> bool {
    let value = cx.heap.with_engine(|engine| engine.get_property(obj, key));
    write_value(cx, out, engine_result(cx, value))
}

/// [[Set]] with the object as receiver, reporting a refusal as TypeError
/// (JS_SetPropertyById's ObjectOpResult::checkStrict).
fn set(cx: &Context, obj: ObjectId, key: PropertyKey, value: JsValue) -> bool {
    let receiver = JsValue::object(obj);
    let name = key_text(&key);
    let done = cx
        .heap
        .with_engine(|engine| engine.set_property_with_receiver(obj, key, value, &receiver));
    match engine_result(cx, done) {
        Some(true) => true,
        Some(false) => throw_type_error(cx, &format!("\"{name}\" is read-only")),
        None => false,
    }
}

fn has_own(cx: &Context, obj: ObjectId, key: PropertyKey, foundp: *mut bool) -> bool {
    let own = cx.heap.with_engine(|engine| engine.own_property(obj, key));
    write_flag(foundp, engine_result(cx, own).map(|d| d.is_some()))
}

/// The class of a `JSObject*` (its shape's base shape).
unsafe fn class_of(obj: *mut root::JSObject) -> *const root::JSClass {
    // SAFETY: an ObjectCell, whose shape and base shape are leaked cells.
    unsafe {
        let shape = object_cell(obj).shadow.shape;
        (*(*shape).base).clasp
    }
}

// --- jsid and linear strings ---

pub unsafe fn int_to_jsid(i: i32, id: JS::MutableHandleId) {
    // SAFETY: the caller's out-parameter.
    unsafe { *id.ptr = IntId(i) };
}

/// An atom's id: an array index in int range becomes an int id
/// (SpiderMonkey's AtomToId).
pub unsafe fn RUST_INTERNED_STRING_TO_JSID(
    cx: *mut root::JSContext,
    str_: *mut root::JSString,
    id: JS::MutableHandleId,
) {
    let cx = unsafe { context(cx) };
    let text = unsafe { string_cell(str_) }.text.clone();
    // SAFETY: the caller's out-parameter.
    unsafe { *id.ptr = id_of(cx, &PropertyKey::String(text)) };
}

pub unsafe fn RUST_JSID_IS_VOID(id: JS::HandleId) -> bool {
    unsafe { (*id.ptr).is_void() }
}

pub unsafe fn RUST_JSID_TO_STRING(id: JS::HandleId) -> *mut root::JSString {
    unsafe { (*id.ptr).to_string() }
}

pub unsafe fn JS_IdToValue(
    cx: *mut root::JSContext,
    id: root::jsid,
    vp: JS::MutableHandleValue,
) -> bool {
    unsafe { context(cx) };
    let value = if id.is_int() {
        Int32Value(id.to_int())
    } else if id.is_string() {
        // SAFETY: an interned StringCell.
        StringValue(unsafe { &*id.to_string() })
    } else if id.is_symbol() {
        // SAFETY: an interned SymbolCell.
        SymbolValue(unsafe { &*id.to_symbol() })
    } else {
        UndefinedValue()
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *vp.ptr = value };
    true
}

/// Atoms and linear strings are the same interned cells.
pub unsafe fn AtomToLinearString(atom: *mut root::JSAtom) -> *mut root::JSLinearString {
    atom as *mut root::JSLinearString
}

pub unsafe fn GetLinearStringLength(s: *mut root::JSLinearString) -> usize {
    unsafe { string_cell(s as *mut root::JSString) }.utf16.len()
}

pub unsafe fn GetLinearStringCharAt(s: *mut root::JSLinearString, idx: usize) -> u16 {
    unsafe { string_cell(s as *mut root::JSString) }.utf16[idx]
}

pub unsafe fn JS_GetStringCharAt(
    cx: *mut root::JSContext,
    str_: *mut root::JSString,
    index: usize,
    res: *mut u16,
) -> bool {
    unsafe { context(cx) };
    let unit = unsafe { string_cell(str_) }.utf16.get(index).copied();
    let Some(unit) = unit else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *res = unit };
    true
}

pub unsafe fn StringIsArrayIndex(str_: *const root::JSLinearString, indexp: *mut u32) -> bool {
    let units = &unsafe { string_cell(str_ as *mut root::JSString) }.utf16;
    match array_index(units.iter().copied()) {
        Some(i) => {
            // SAFETY: the caller's out-parameter.
            unsafe { *indexp = i };
            true
        }
        None => false,
    }
}

pub unsafe fn StringIsArrayIndex1(str_: *const u16, length: u32, indexp: *mut u32) -> bool {
    let units: &[u16] = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller's `length` UTF-16 units.
        unsafe { std::slice::from_raw_parts(str_, length as usize) }
    };
    match array_index(units.iter().copied()) {
        Some(i) => {
            // SAFETY: the caller's out-parameter.
            unsafe { *indexp = i };
            true
        }
        None => false,
    }
}

pub unsafe fn JS_AtomizeStringN(
    cx: *mut root::JSContext,
    s: *const c_char,
    length: usize,
) -> *mut root::JSString {
    let cx = unsafe { context(cx) };
    let bytes: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller's `length` Latin-1 bytes.
        unsafe { std::slice::from_raw_parts(s as *const u8, length) }
    };
    cx.heap
        .atoms
        .string(bytes.iter().map(|&b| b as char).collect())
}

pub unsafe fn JS_GetEmptyString(cx: *mut root::JSContext) -> *mut root::JSString {
    unsafe { context(cx) }.heap.atoms.string(String::new())
}

pub unsafe fn JS_ConcatStrings(
    cx: *mut root::JSContext,
    left: JS::Handle<*mut root::JSString>,
    right: JS::Handle<*mut root::JSString>,
) -> *mut root::JSString {
    let cx = unsafe { context(cx) };
    let (l, r) = unsafe { (string_cell(*left.ptr), string_cell(*right.ptr)) };
    let units: Vec<u16> = l.utf16.iter().chain(r.utf16.iter()).copied().collect();
    cx.heap.atoms.string(String::from_utf16_lossy(&units))
}

/// Code-unit order, as SpiderMonkey's CompareStrings (and ECMA-262 §7.2.13
/// IsLessThan on strings).
pub unsafe fn JS_CompareStrings(
    cx: *mut root::JSContext,
    s1: *mut root::JSString,
    s2: *mut root::JSString,
    result: *mut i32,
) -> bool {
    unsafe { context(cx) };
    let (a, b) = unsafe { (&string_cell(s1).utf16, &string_cell(s2).utf16) };
    // SAFETY: the caller's out-parameter.
    unsafe { *result = a.cmp(b) as i32 };
    true
}

pub unsafe fn JS_StringEqualsAscii(
    cx: *mut root::JSContext,
    str_: *mut root::JSString,
    asciiBytes: *const c_char,
    match_: *mut bool,
) -> bool {
    unsafe { context(cx) };
    let units = &unsafe { string_cell(str_) }.utf16;
    // SAFETY: a NUL-terminated ASCII string from the embedder.
    let bytes = unsafe { CStr::from_ptr(asciiBytes) }.to_bytes();
    let equal = units.len() == bytes.len() && units.iter().zip(bytes).all(|(&u, &b)| u == b as u16);
    // SAFETY: the caller's out-parameter.
    unsafe { *match_ = equal };
    true
}

// --- Id vectors ---

/// A `JS::PersistentRootedIdVector`.
pub struct IdVectorCell(RefCell<Vec<root::jsid>>);

unsafe fn id_vector<'a>(v: *const c_void) -> &'a IdVectorCell {
    assert!(!v.is_null(), "mozjs facade: null IdVector");
    // SAFETY: IdVector pointers come from CreateRootedIdVector only.
    unsafe { &*(v as *const IdVectorCell) }
}

pub unsafe fn CreateRootedIdVector(cx: *mut root::JSContext) -> *mut JS::PersistentRootedIdVector {
    unsafe { context(cx) };
    Box::into_raw(Box::new(IdVectorCell(RefCell::new(Vec::new()))))
        as *mut JS::PersistentRootedIdVector
}

/// The vector is its own address (MutableHandleIdVector::ptr).
pub unsafe fn GetIdVectorAddress(v: *mut JS::PersistentRootedIdVector) -> *mut c_void {
    v as *mut c_void
}

pub unsafe fn SliceRootedIdVector(
    v: *const JS::PersistentRootedIdVector,
    length: *mut usize,
) -> *const root::jsid {
    let ids = unsafe { id_vector(v as *const c_void) }.0.borrow();
    // SAFETY: the caller's out-parameter.
    unsafe { *length = ids.len() };
    ids.as_ptr()
}

pub unsafe fn DestroyRootedIdVector(v: *mut JS::PersistentRootedIdVector) {
    if !v.is_null() {
        // SAFETY: made by CreateRootedIdVector and destroyed once.
        drop(unsafe { Box::from_raw(v as *mut IdVectorCell) });
    }
}

pub unsafe fn AppendToIdVector(v: JS::MutableHandleIdVector, id: JS::HandleId) -> bool {
    unsafe { id_vector(v.ptr) }
        .0
        .borrow_mut()
        .push(unsafe { *id.ptr });
    true
}

/// The keys `for-in` and `Reflect.ownKeys` see, by `JSITER_*` flags
/// (jsfriendapi GetPropertyKeys): own keys in [[OwnPropertyKeys]] order
/// (§10.1.11), then each prototype's keys not already seen
/// (§14.7.5.9 EnumerateObjectProperties).
pub unsafe fn GetPropertyKeys(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    flags: c_uint,
    props: JS::MutableHandleIdVector,
) -> bool {
    let cx = unsafe { context(cx) };
    let obj = unsafe { handle_object(obj) };
    let own_only = flags & root::JSITER_OWNONLY != 0;
    let hidden = flags & root::JSITER_HIDDEN != 0;
    let symbols_only = flags & root::JSITER_SYMBOLSONLY != 0;
    let symbols = symbols_only || flags & root::JSITER_SYMBOLS != 0;
    let keys = cx.heap.with_engine(|engine| {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut current = Some(obj);
        while let Some(object) = current {
            for key in engine.own_property_keys(object)? {
                let is_symbol = matches!(key, PropertyKey::Symbol(_));
                if (is_symbol && !symbols) || (!is_symbol && symbols_only) {
                    continue;
                }
                if !seen.insert(key.clone()) {
                    continue;
                }
                let Some(desc) = engine.own_property(object, key.clone())? else {
                    continue;
                };
                if hidden || desc.enumerable {
                    out.push(key);
                }
            }
            if own_only {
                break;
            }
            current = engine.prototype_of(object)?;
        }
        Ok(out)
    });
    let Some(keys) = engine_result(cx, keys) else {
        return false;
    };
    let mut ids = unsafe { id_vector(props.ptr) }.0.borrow_mut();
    ids.extend(keys.iter().map(|k| id_of(cx, k)));
    true
}

// --- Property access by id ---

pub unsafe fn JS_GetPropertyById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    vp: JS::MutableHandleValue,
) -> bool {
    let cx = unsafe { context(cx) };
    unsafe { get(cx, handle_object(obj), key_of(*id.ptr), vp.ptr) }
}

pub unsafe fn JS_GetElement(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    index: u32,
    vp: JS::MutableHandleValue,
) -> bool {
    let cx = unsafe { context(cx) };
    let key = PropertyKey::String(index.to_string());
    get(cx, unsafe { handle_object(obj) }, key, vp.ptr)
}

pub unsafe fn JS_GetUCProperty(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    name: *const u16,
    namelen: usize,
    vp: JS::MutableHandleValue,
) -> bool {
    let cx = unsafe { context(cx) };
    let units: &[u16] = if namelen == 0 {
        &[]
    } else {
        // SAFETY: the caller's `namelen` UTF-16 units.
        unsafe { std::slice::from_raw_parts(name, namelen) }
    };
    let key = PropertyKey::String(String::from_utf16_lossy(units));
    get(cx, unsafe { handle_object(obj) }, key, vp.ptr)
}

/// [[Get]] with a different receiver (§10.1.8 with Receiver).
pub unsafe fn JS_ForwardGetPropertyTo(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    receiver: JS::Handle<JS::Value>,
    vp: JS::MutableHandleValue,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key, receiver) =
        unsafe { (handle_object(obj), key_of(*id.ptr), handle_jsval(receiver)) };
    let value = cx
        .heap
        .with_engine(|engine| engine.get_property_with_receiver(obj, key, &receiver));
    write_value(cx, vp.ptr, engine_result(cx, value))
}

pub unsafe fn JS_SetProperty(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    name: *const c_char,
    v: JS::Handle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    let key = PropertyKey::String(unsafe { latin1(name) });
    unsafe { set(cx, handle_object(obj), key, handle_jsval(v)) }
}

pub unsafe fn JS_SetPropertyById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    v: JS::Handle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    unsafe { set(cx, handle_object(obj), key_of(*id.ptr), handle_jsval(v)) }
}

pub unsafe fn JS_SetElement(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    index: u32,
    v: JS::Handle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    let key = PropertyKey::String(index.to_string());
    unsafe { set(cx, handle_object(obj), key, handle_jsval(v)) }
}

pub unsafe fn JS_HasPropertyById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    foundp: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key) = unsafe { (handle_object(obj), key_of(*id.ptr)) };
    let found = cx.heap.with_engine(|engine| engine.has_property(obj, key));
    write_flag(foundp, engine_result(cx, found))
}

pub unsafe fn JS_HasOwnPropertyById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    foundp: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    unsafe { has_own(cx, handle_object(obj), key_of(*id.ptr), foundp) }
}

pub unsafe fn JS_HasOwnProperty(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    name: *const c_char,
    foundp: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let key = PropertyKey::String(unsafe { latin1(name) });
    has_own(cx, unsafe { handle_object(obj) }, key, foundp)
}

/// No resolve hooks run in the facade, so this is [[GetOwnProperty]].
pub unsafe fn JS_AlreadyHasOwnPropertyById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    foundp: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    unsafe { has_own(cx, handle_object(obj), key_of(*id.ptr), foundp) }
}

pub unsafe fn JS_DeletePropertyById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    result: *mut JS::ObjectOpResult,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key) = unsafe { (handle_object(obj), key_of(*id.ptr)) };
    let deleted = cx
        .heap
        .with_engine(|engine| engine.delete_property(obj, key));
    unsafe {
        op_result(
            result,
            engine_result(cx, deleted),
            root::JSErrNum::JSMSG_CANT_DELETE,
        )
    }
}

pub unsafe fn JS_DeleteProperty(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    name: *const c_char,
    result: *mut JS::ObjectOpResult,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key) = unsafe { (handle_object(obj), PropertyKey::String(latin1(name))) };
    let deleted = cx
        .heap
        .with_engine(|engine| engine.delete_property(obj, key));
    unsafe {
        op_result(
            result,
            engine_result(cx, deleted),
            root::JSErrNum::JSMSG_CANT_DELETE,
        )
    }
}

// --- Descriptors ---

pub unsafe fn JS_GetOwnPropertyDescriptorById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    desc: JS::MutableHandle<JS::PropertyDescriptor>,
    isNone: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key) = unsafe { (handle_object(obj), key_of(*id.ptr)) };
    let found = cx.heap.with_engine(|engine| {
        let own = engine.own_property(obj, key)?;
        Ok(own.map(|d| to_descriptor(cx, engine, &d)))
    });
    let Some(found) = engine_result(cx, found) else {
        return false;
    };
    // SAFETY: the caller's out-parameters.
    unsafe {
        *isNone = found.is_none();
        *desc.ptr = found.unwrap_or_else(blank_descriptor);
    }
    true
}

/// The descriptor found on the object or its prototype chain, and the
/// object holding it.
pub unsafe fn JS_GetPropertyDescriptorById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    desc: JS::MutableHandle<JS::PropertyDescriptor>,
    holder: JS::MutableHandleObject,
    isNone: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key) = unsafe { (handle_object(obj), key_of(*id.ptr)) };
    let found = cx.heap.with_engine(|engine| {
        let mut current = Some(obj);
        while let Some(object) = current {
            if let Some(d) = engine.own_property(object, key.clone())? {
                let holder = cx.heap.object(cx, engine, object, ptr::null());
                return Ok(Some((to_descriptor(cx, engine, &d), holder)));
            }
            current = engine.prototype_of(object)?;
        }
        Ok(None)
    });
    let Some(found) = engine_result(cx, found) else {
        return false;
    };
    // SAFETY: the caller's out-parameters.
    unsafe {
        *isNone = found.is_none();
        match found {
            Some((d, h)) => {
                *desc.ptr = d;
                *holder.ptr = h;
            }
            None => {
                *desc.ptr = blank_descriptor();
                *holder.ptr = ptr::null_mut();
            }
        }
    }
    true
}

pub unsafe fn SetDataPropertyDescriptor(
    desc: JS::MutableHandle<JS::PropertyDescriptor>,
    value: JS::HandleValue,
    attrs: u32,
) {
    let (w, e, c) = attributes(attrs as c_uint);
    let mut d = blank_descriptor();
    d.set_hasConfigurable_(true);
    d.set_configurable_(c);
    d.set_hasEnumerable_(true);
    d.set_enumerable_(e);
    d.set_hasWritable_(true);
    d.set_writable_(w);
    d.set_hasValue_(true);
    // SAFETY: the caller's rooted value and out-parameter.
    unsafe {
        d.value_ = *value.ptr;
        *desc.ptr = d;
    }
}

pub unsafe fn JS_DefinePropertyById(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    desc: JS::Handle<JS::PropertyDescriptor>,
    result: *mut JS::ObjectOpResult,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key, desc) = unsafe {
        (
            handle_object(obj),
            key_of(*id.ptr),
            from_descriptor(&*desc.ptr),
        )
    };
    let defined = define(cx, obj, key, desc);
    unsafe { op_result(result, defined, root::JSErrNum::JSMSG_CANT_REDEFINE_PROP) }
}

/// Like JS_DefinePropertyById, with a refusal reported as TypeError
/// (§7.3.8 DefinePropertyOrThrow).
pub unsafe fn JS_DefinePropertyById1(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    desc: JS::Handle<JS::PropertyDescriptor>,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key, desc) = unsafe {
        (
            handle_object(obj),
            key_of(*id.ptr),
            from_descriptor(&*desc.ptr),
        )
    };
    let name = key_text(&key);
    match define(cx, obj, key, desc) {
        Some(true) => true,
        Some(false) => throw_type_error(
            cx,
            &format!("can't redefine non-configurable property \"{name}\""),
        ),
        None => false,
    }
}

pub unsafe fn JS_DefinePropertyById3(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    getter: root::JSNative,
    setter: root::JSNative,
    attrs: c_uint,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key) = unsafe { (handle_object(obj), key_of(*id.ptr)) };
    let name = key_text(&key);
    define_native_accessor(
        cx,
        obj,
        key,
        &name,
        native_wrapper(getter),
        native_wrapper(setter),
        attrs,
    )
}

/// OrdinarySetWithOwnDescriptor (ECMA-262 §10.1.9.2) with the caller's
/// `ownDesc`: DOM proxies set through it so their named getter is skipped.
pub unsafe fn SetPropertyIgnoringNamedGetter(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    id: JS::HandleId,
    v: JS::HandleValue,
    receiver: JS::HandleValue,
    ownDesc: *const JS::Handle<JS::PropertyDescriptor>,
    result: *mut JS::ObjectOpResult,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, key, value, receiver) = unsafe {
        (
            handle_object(obj),
            key_of(*id.ptr),
            handle_jsval(v),
            handle_jsval(receiver),
        )
    };
    let own = unsafe { from_descriptor(&*(*ownDesc).ptr) };
    let outcome = cx.heap.with_engine(|engine| {
        let is_accessor = own.get.is_some() || own.set.is_some();
        // Step 2: no own property; defer to the parent, else a fresh data
        // property { [[Writable]]: true, ... }.
        if !is_accessor && own.value.is_none() && own.writable.is_none() {
            if let Some(parent) = engine.prototype_of(obj)? {
                return engine.set_property_with_receiver(parent, key, value, &receiver);
            }
        }
        if !is_accessor {
            // Step 2.a.
            if own.writable == Some(false) {
                return Ok(false);
            }
            // Step 2.b.
            let Some(receiver) = receiver.as_object() else {
                return Ok(false);
            };
            // Steps 2.c-2.e.
            match engine.own_property(receiver, key.clone())? {
                Some(existing) => {
                    if existing.value.is_none() || !existing.writable {
                        return Ok(false);
                    }
                    let update = PartialDescriptor {
                        value: Some(value),
                        ..PartialDescriptor::default()
                    };
                    engine.define_own_property(receiver, key, update)
                }
                None => engine.define_property(receiver, key, value, true, true, true),
            }
        } else {
            // Steps 3-7.
            let Some(Some(setter)) = own.set else {
                return Ok(false);
            };
            engine.call_function(&JsValue::object(setter), &receiver, &[value])?;
            Ok(true)
        }
    });
    unsafe {
        op_result(
            result,
            engine_result(cx, outcome),
            root::JSErrNum::JSMSG_READ_ONLY,
        )
    }
}

// --- Objects ---

pub unsafe fn GetObjectProto(
    cx: *mut root::JSContext,
    obj: JS::HandleObject,
    proto: JS::MutableHandleObject,
) -> bool {
    let cx = unsafe { context(cx) };
    let obj = unsafe { handle_object(obj) };
    let found = cx.heap.with_engine(|engine| engine.prototype_of(obj));
    let Some(found) = engine_result(cx, found) else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *proto.ptr = object_ptr(cx, found) };
    true
}

/// Every facade object's prototype is static: proxies keep theirs in the
/// engine too.
pub unsafe fn GetStaticPrototype(obj: *mut root::JSObject) -> *mut root::JSObject {
    let cell = unsafe { object_cell(obj) };
    let cx = cell.context();
    let proto = cx.heap.with_engine(|engine| engine.prototype_of(cell.id));
    object_ptr(cx, engine_result(cx, proto).flatten())
}

/// SetIntegrityLevel(O, frozen) (ECMA-262 §7.3.15), throwing when refused
/// (Object.freeze, §20.1.2.6).
pub unsafe fn JS_FreezeObject(cx: *mut root::JSContext, obj: JS::HandleObject) -> bool {
    let cx = unsafe { context(cx) };
    let obj = unsafe { handle_object(obj) };
    let frozen = cx.heap.with_engine(|engine| engine.freeze_object(obj));
    match engine_result(cx, frozen) {
        Some(true) => true,
        Some(false) => throw_type_error(cx, "can't freeze object"),
        None => false,
    }
}

pub unsafe fn SameValue(
    cx: *mut root::JSContext,
    v1: JS::Handle<JS::Value>,
    v2: JS::Handle<JS::Value>,
    same: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let (a, b) = unsafe { (handle_jsval(v1), handle_jsval(v2)) };
    let equal = cx.heap.with_engine(|engine| engine.same_value(&a, &b));
    write_flag(same, Some(equal))
}

pub unsafe fn IsArrayObject(
    cx: *mut root::JSContext,
    value: JS::Handle<JS::Value>,
    isArray: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let value = unsafe { handle_jsval(value) };
    let is = cx.heap.with_engine(|engine| engine.is_array_value(&value));
    write_flag(isArray, engine_result(cx, is))
}

pub unsafe fn IsArrayObject1(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    isArray: *mut bool,
) -> bool {
    let cx = unsafe { context(cx) };
    let value = JsValue::object(unsafe { handle_object(obj) });
    let is = cx.heap.with_engine(|engine| engine.is_array_value(&value));
    write_flag(isArray, engine_result(cx, is))
}

/// GetFunctionRealm (ECMA-262 §7.3.24).
pub unsafe fn GetFunctionRealm(
    cx: *mut root::JSContext,
    objArg: JS::HandleObject,
) -> *mut JS::Realm {
    let cx = unsafe { context(cx) };
    let function = JsValue::object(unsafe { handle_object(objArg) });
    let realm = cx
        .heap
        .with_engine(|engine| engine.function_realm_id(&function));
    match engine_result(cx, realm) {
        Some(realm) => cx.heap.realm_ptr(realm),
        None => ptr::null_mut(),
    }
}

/// The WindowProxy is the object of the class Servo registered with
/// SetWindowProxyClass.
pub unsafe fn IsWindowProxy(obj: *mut root::JSObject) -> bool {
    let class = unsafe { GetWindowProxyClass() };
    !class.is_null() && unsafe { class_of(obj) } == class
}

/// Wrappers are the identity (natives.rs), so none is ever dead.
pub unsafe fn JS_IsDeadWrapper(_obj: *mut root::JSObject) -> bool {
    false
}

pub unsafe fn UnwrapObjectDynamic(
    obj: *mut root::JSObject,
    _cx: *mut root::JSContext,
    _stopAtWindowProxy: bool,
) -> *mut root::JSObject {
    obj
}

// --- Functions and calls ---

fn callable(obj: *mut root::JSObject) -> bool {
    let cell = unsafe { object_cell(obj) };
    let cx = cell.context();
    cx.heap
        .with_engine(|engine| engine.is_callable(&JsValue::object(cell.id)))
}

/// IsCallable (ECMA-262 §7.2.3).
pub unsafe fn IsCallable(obj: *mut root::JSObject) -> bool {
    callable(obj)
}

/// Every callable facade object is a function object (the engine has no
/// callable non-function hosts).
pub unsafe fn JS_ObjectIsFunction(obj: *mut root::JSObject) -> bool {
    callable(obj)
}

pub unsafe fn JS_GetObjectFunction(obj: *mut root::JSObject) -> *mut root::JSFunction {
    if callable(obj) {
        obj as *mut root::JSFunction
    } else {
        ptr::null_mut()
    }
}

pub unsafe fn JS_IsNativeFunction(funobj: *mut root::JSObject, call: root::JSNative) -> bool {
    let cell = unsafe { object_cell(funobj) };
    let shared = &cell.context().heap.shared;
    let word = shared.native_of.borrow().get(&cell.id).copied();
    word.is_some_and(|w| {
        shared.natives.borrow()[w].op.map(|f| f as usize) == call.map(|f| f as usize)
    })
}

fn call(
    cx: &Context,
    function: &JsValue,
    this: &JsValue,
    args: &[JsValue],
    rval: *mut JS::Value,
) -> bool {
    let value = cx
        .heap
        .with_engine(|engine| engine.call_function(function, this, args));
    write_value(cx, rval, engine_result(cx, value))
}

/// Call (ECMA-262 §7.3.14).
pub unsafe fn Call(
    cx: *mut root::JSContext,
    thisv: JS::Handle<JS::Value>,
    fun: JS::Handle<JS::Value>,
    args: *const JS::HandleValueArray,
    rval: JS::MutableHandle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    let (this, fun, args) = unsafe { (handle_jsval(thisv), handle_jsval(fun), value_array(args)) };
    call(cx, &fun, &this, &args, rval.ptr)
}

pub unsafe fn JS_CallFunctionValue(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    fval: JS::Handle<JS::Value>,
    args: *const JS::HandleValueArray,
    rval: JS::MutableHandle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    let this =
        unsafe { handle_object_or_null(obj) }.map_or_else(JsValue::undefined, JsValue::object);
    let (fun, args) = unsafe { (handle_jsval(fval), value_array(args)) };
    call(cx, &fun, &this, &args, rval.ptr)
}

pub unsafe fn JS_CallFunctionName(
    cx: *mut root::JSContext,
    obj: JS::Handle<*mut root::JSObject>,
    name: *const c_char,
    args: *const JS::HandleValueArray,
    rval: JS::MutableHandle<JS::Value>,
) -> bool {
    let cx = unsafe { context(cx) };
    let (obj, name, args) = unsafe { (handle_object(obj), latin1(name), value_array(args)) };
    let fun = cx
        .heap
        .with_engine(|engine| engine.get_property(obj, name.as_str()));
    let Some(fun) = engine_result(cx, fun) else {
        return false;
    };
    call(cx, &fun, &JsValue::object(obj), &args, rval.ptr)
}

fn construct(
    cx: &Context,
    fun: &JsValue,
    new_target: Option<ObjectId>,
    args: &[JsValue],
    objp: *mut *mut root::JSObject,
) -> bool {
    let result = cx.heap.with_engine(|engine| {
        if new_target.is_some_and(|t| fun.as_object() != Some(t)) {
            return Err(engine.throw_new_error(
                "TypeError",
                "mozjs facade: Construct with a different new.target is not supported yet",
            ));
        }
        let value = engine.construct_object(fun, args)?;
        Ok(value
            .as_object()
            .map(|id| cx.heap.object(cx, engine, id, ptr::null())))
    });
    let Some(object) = engine_result(cx, result).flatten() else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *objp = object };
    true
}

/// Construct (ECMA-262 §7.3.15).
pub unsafe fn Construct(
    cx: *mut root::JSContext,
    fun: JS::Handle<JS::Value>,
    newTarget: JS::Handle<*mut root::JSObject>,
    args: *const JS::HandleValueArray,
    objp: JS::MutableHandle<*mut root::JSObject>,
) -> bool {
    let cx = unsafe { context(cx) };
    let (fun, target, args) = unsafe {
        (
            handle_jsval(fun),
            handle_object_or_null(newTarget),
            value_array(args),
        )
    };
    construct(cx, &fun, target, &args, objp.ptr)
}

pub unsafe fn Construct1(
    cx: *mut root::JSContext,
    fun: JS::Handle<JS::Value>,
    args: *const JS::HandleValueArray,
    objp: JS::MutableHandle<*mut root::JSObject>,
) -> bool {
    let cx = unsafe { context(cx) };
    let (fun, args) = unsafe { (handle_jsval(fun), value_array(args)) };
    construct(cx, &fun, None, &args, objp.ptr)
}

// --- Conversions ---

/// ToString (ECMA-262 §7.1.17).
pub unsafe fn ToStringSlow(cx: *mut root::JSContext, v: JS::HandleValue) -> *mut root::JSString {
    let cx = unsafe { context(cx) };
    let value = unsafe { handle_jsval(v) };
    let text = cx.heap.with_engine(|engine| engine.to_string_value(&value));
    match engine_result(cx, text) {
        Some(text) => cx.heap.atoms.string(text),
        None => ptr::null_mut(),
    }
}

/// ToBoolean (ECMA-262 §7.1.2).
pub unsafe fn ToBooleanSlow(v: JS::HandleValue) -> bool {
    // SAFETY: a Handle points to a rooted, live location.
    let v: JSVal = unsafe { *v.ptr };
    if v.is_boolean() {
        v.to_boolean()
    } else if v.is_number() {
        let n = v.to_number();
        n != 0.0 && !n.is_nan()
    } else if v.is_string() {
        !unsafe { string_cell(v.to_string()) }.utf16.is_empty()
    } else {
        !(v.is_undefined() || v.is_null())
    }
}

/// The realm the context has entered, or null outside every realm.
pub unsafe fn GetCurrentRealmOrNull(cx: *mut root::JSContext) -> *mut JS::Realm {
    unsafe { context(cx) }.realm()
}
