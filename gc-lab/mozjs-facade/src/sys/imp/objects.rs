/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Objects, realms, compartments and global creation (MOZJS_FACADE.md
//! "Engine ownership, reentrancy and object cells").
//!
//! The engine owns its objects; Servo sees them as `JSObject*`. Servo's Rust
//! layer reads `obj->shape->base->{clasp, realm}` inline (`get_object_class`,
//! `get_object_realm` in rust.rs), so a `JSObject*` must be the address of
//! real memory with SpiderMonkey's shadow layout. Each engine object handed
//! out gets an [`ObjectCell`]: a `JS::shadow::Object` whose shape is an
//! interned [`ShapeCell`] per (class, realm), followed by the owning context
//! and the engine's `ObjectId`. Entry points that receive only a `JSObject*`
//! (`JS_GetReservedSlot`) find their engine through the cell.
//!
//! A `JS::Realm*` is a [`RealmCell`] and a `JS::Compartment*` a
//! [`CompartmentCell`]. All compartments of a context share its single engine:
//! the engine has no cross-compartment wrappers, so a compartment only
//! records the system flag Servo asks about.
//!
//! C1 lifetime policy: cells are never freed and every object with a cell is
//! reported as a root (`Host::trace_roots`). That keeps every `JSObject*`
//! Servo holds valid until tracing (SERVO_BINDINGS.md E4) is connected to
//! Servo's trace hooks; then cells of dead objects are dropped after each
//! collection.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::raw::c_void;
use std::ptr;
use std::rc::Rc;

use gc_lab::script::{Engine, Host, JsValue, ObjectId, RealmId, ScriptError};
use gc_lab::Model;

use super::context::{context, Context};
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;
use crate::sys::jsval::{JSVal, UndefinedValue};

/// What a context owns besides its callbacks.
#[derive(Default)]
pub struct Heap {
    /// Created on the first global (`JS_NewGlobalObject`). Empty while an
    /// entry point runs it ([`Heap::with_engine`] takes it out); a JSNative
    /// call parks it back here while the native runs (natives.rs).
    pub(super) engine: RefCell<Option<Engine<'static>>>,
    engine_created: Cell<bool>,
    /// Hostless engines swapped into the caller's `&mut Engine` while the real
    /// one is parked; one per JSNative nesting depth, reused.
    pub(super) spares: RefCell<Vec<Engine<'static>>>,
    pub(super) shared: Rc<Shared>,
    pub(super) atoms: super::natives::Atoms,
    /// AddAssociatedMemory/RemoveAssociatedMemory balance, in bytes.
    pub(super) associated_bytes: Cell<usize>,
    shapes: RefCell<HashMap<(usize, usize), &'static ShapeCell>>,
    realms: RefCell<HashMap<RealmId, &'static RealmCell>>,
    compartments: RefCell<Vec<&'static CompartmentCell>>,
    trusted_principals: Cell<usize>,
}

/// State the engine's [`Host`] reads during a collection.
#[derive(Default)]
pub(super) struct Shared {
    /// The owning `Context` (set by JS_NewContext): JSNative dispatch needs the
    /// `JSContext*` and the heap.
    pub(super) context: Cell<usize>,
    /// Host functions (JS_NewFunction, spec methods and accessors), indexed by
    /// the engine's host function word.
    pub(super) natives: RefCell<Vec<super::natives::Native>>,
    pub(super) native_of: RefCell<HashMap<ObjectId, usize>>,
    cells: RefCell<HashMap<ObjectId, &'static ObjectCell>>,
    /// Reserved slots, bit-exact (`PrivateValue` doubles and int32 values keep
    /// their tags), keyed by object.
    slots: RefCell<HashMap<ObjectId, Vec<JSVal>>>,
}

#[repr(C)]
pub struct ObjectCell {
    /// Must stay first: this is what `JSObject*` points to.
    shadow: JS::shadow::Object,
    context: *const Context,
    pub(super) id: ObjectId,
}

#[repr(C)]
pub struct ShapeCell {
    shape: JS::shadow::Shape,
    base: JS::shadow::BaseShape,
}

pub struct RealmCell {
    context: *const Context,
    id: RealmId,
    global: Cell<*mut root::JSObject>,
    principals: *mut root::JSPrincipals,
    compartment: &'static CompartmentCell,
}

pub struct CompartmentCell {
    context: *const Context,
    system: bool,
}

/// The engine's view of the embedder: JSNative dispatch (natives.rs), roots
/// and reserved-slot edges for the collector.
struct FacadeHost(Rc<Shared>);

impl Host for FacadeHost {
    fn call(
        &self,
        engine: &mut Engine<'_>,
        function: u64,
        this: JsValue,
        args: &[JsValue],
        new_target: Option<ObjectId>,
    ) -> Result<JsValue, ScriptError> {
        super::natives::call_native(&self.0, engine, function, this, args, new_target)
    }

    fn trace_roots(&self, out: &mut Vec<ObjectId>) {
        out.extend(self.0.cells.borrow().keys().copied());
    }

    fn trace(&self, object: ObjectId, _class: u64, _payload: u64, out: &mut Vec<ObjectId>) {
        if let Some(slots) = self.0.slots.borrow().get(&object) {
            for value in slots.iter().filter(|v| v.is_object()) {
                // SAFETY: an object value in a reserved slot is a cell pointer.
                out.push(unsafe { object_cell(value.to_object()) }.id);
            }
        }
    }
}

/// A class for engine objects that Servo did not create with a `JSClass`.
static PLAIN_OBJECT_CLASS: root::JSClass = root::JSClass {
    name: c"Object".as_ptr(),
    flags: 0,
    cOps: ptr::null(),
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

/// `JSCLASS_RESERVED_SLOTS(clasp)`.
fn reserved_slot_count(clasp: *const root::JSClass) -> usize {
    if clasp.is_null() {
        return 0;
    }
    // SAFETY: a non-null JSClass from the embedder, valid for the process.
    let flags = unsafe { (*clasp).flags };
    ((flags >> root::JSCLASS_RESERVED_SLOTS_SHIFT) & root::JSCLASS_RESERVED_SLOTS_MASK) as usize
}

/// The cell behind a `JSObject*` the facade handed out.
///
/// # Safety
/// `obj` must be non-null and come from this facade.
pub unsafe fn object_cell<'a>(obj: *mut root::JSObject) -> &'a ObjectCell {
    assert!(!obj.is_null(), "mozjs facade: null JSObject");
    // SAFETY: by the contract, `obj` is a leaked ObjectCell.
    unsafe { &*(obj as *const ObjectCell) }
}

unsafe fn realm_cell<'a>(realm: *mut JS::Realm) -> &'a RealmCell {
    assert!(!realm.is_null(), "mozjs facade: null Realm");
    // SAFETY: realm pointers come from RealmCell leaks only.
    unsafe { &*(realm as *const RealmCell) }
}

impl ObjectCell {
    pub(super) fn context(&self) -> &Context {
        // SAFETY: a context outlives the objects it created; Servo destroys
        // its contexts last.
        unsafe { &*self.context }
    }

    fn realm(&self) -> *mut JS::Realm {
        // SAFETY: the shape and base shape are leaked cells.
        unsafe { (*self.shadow.shape.cast::<ShapeCell>()).base.realm }
    }
}

impl Heap {
    /// Run `f` on the engine, creating it first if needed. The engine is
    /// taken out of its slot while `f` runs, so a JSNative that `f` reaches
    /// can park it back (natives.rs) and reenter through another entry point.
    pub(super) fn with_engine<R>(&self, f: impl FnOnce(&mut Engine<'static>) -> R) -> R {
        let parked = self.engine.borrow_mut().take();
        let mut engine = match parked {
            Some(engine) => engine,
            None => {
                assert!(
                    !self.engine_created.get(),
                    "mozjs facade: engine reentered outside a JSNative call"
                );
                self.engine_created.set(true);
                let mut engine = Engine::new(Model::Indirect, Model::Indirect)
                    .expect("mozjs facade: engine creation");
                engine.set_host(Rc::new(FacadeHost(self.shared.clone())));
                engine
            }
        };
        let result = f(&mut engine);
        *self.engine.borrow_mut() = Some(engine);
        result
    }

    /// A hostless engine to stand in for the running one during a JSNative.
    pub(super) fn take_spare(&self) -> Engine<'static> {
        self.spares.borrow_mut().pop().unwrap_or_else(|| {
            Engine::new(Model::Indirect, Model::Indirect).expect("mozjs facade: engine creation")
        })
    }

    fn shape(&self, clasp: *const root::JSClass, realm: *mut JS::Realm) -> &'static ShapeCell {
        let clasp = if clasp.is_null() {
            &PLAIN_OBJECT_CLASS
        } else {
            clasp
        };
        *self
            .shapes
            .borrow_mut()
            .entry((clasp as usize, realm as usize))
            .or_insert_with(|| {
                let cell: &'static mut ShapeCell = Box::leak(Box::new(ShapeCell {
                    shape: JS::shadow::Shape {
                        base: ptr::null_mut(),
                        immutableFlags: 0,
                    },
                    base: JS::shadow::BaseShape { clasp, realm },
                }));
                cell.shape.base = &mut cell.base;
                cell
            })
    }

    /// The `JSObject*` of `id`, made on first use with class `clasp` (null:
    /// the host class recorded in the engine, else a plain object class).
    pub(super) fn object(
        &self,
        cx: &Context,
        engine: &Engine<'_>,
        id: ObjectId,
        clasp: *const root::JSClass,
    ) -> *mut root::JSObject {
        if let Some(cell) = self.shared.cells.borrow().get(&id) {
            return *cell as *const ObjectCell as *mut root::JSObject;
        }
        let clasp = if clasp.is_null() {
            engine.host_class(id).map_or(ptr::null(), |(class, _)| {
                class as usize as *const root::JSClass
            })
        } else {
            clasp
        };
        let realm = self.realm_ptr(engine.object_realm(id));
        let shape = self.shape(clasp, realm);
        let cell: &'static ObjectCell = Box::leak(Box::new(ObjectCell {
            shadow: JS::shadow::Object {
                shape: &shape.shape as *const JS::shadow::Shape as *mut JS::shadow::Shape,
            },
            context: cx,
            id,
        }));
        self.shared.cells.borrow_mut().insert(id, cell);
        cell as *const ObjectCell as *mut root::JSObject
    }

    fn realm_ptr(&self, id: RealmId) -> *mut JS::Realm {
        self.realms.borrow().get(&id).map_or(ptr::null_mut(), |r| {
            *r as *const RealmCell as *mut JS::Realm
        })
    }

    pub(super) fn init_slots(&self, id: ObjectId, clasp: *const root::JSClass) {
        let count = reserved_slot_count(clasp);
        self.shared
            .slots
            .borrow_mut()
            .insert(id, vec![UndefinedValue(); count]);
    }
}

pub(super) fn engine_result<T>(cx: &Context, result: Result<T, ScriptError>) -> Option<T> {
    match result {
        Ok(v) => Some(v),
        Err(error) => {
            // A thrown completion becomes the context's pending exception, as
            // SpiderMonkey leaves it for JS_GetPendingException. Other errors
            // (engine faults, Unsupported) are uncatchable, like OOM: logged,
            // with no exception pending.
            let pending = cx.heap.with_engine(|engine| {
                engine
                    .exception_value(&error)
                    .map(|v| super::natives::to_jsval(cx, engine, &v))
            });
            match pending {
                Some(value) => cx.state.borrow_mut().pending_exception = Some(value),
                None => eprintln!("mozjs facade: engine error: {error:?}"),
            }
            None
        }
    }
}

// --- Globals and realms (js/public/Realm.h, jsapi.h) ---

pub unsafe fn JS_NewGlobalObject(
    cx: *mut root::JSContext,
    clasp: *const root::JSClass,
    principals: *mut root::JSPrincipals,
    _hook_option: JS::OnNewGlobalHookOption,
    options: *const JS::RealmOptions,
) -> *mut root::JSObject {
    let context = unsafe { context(cx) };
    let heap = &context.heap;
    // CompartmentSpecifier: reuse the given compartment, else a new one. A
    // realm created with the trusted principals is a system realm, and its
    // compartment a system compartment (Realm::isSystem).
    let existing = (!options.is_null())
        .then(|| {
            // SAFETY: Servo passes a live RealmOptions.
            let creation = unsafe { &(*options).creationOptions_ };
            (creation.compSpec_ == JS::CompartmentSpecifier::ExistingCompartment)
                // SAFETY: comp_ is the active member for ExistingCompartment.
                .then(|| unsafe { creation.__bindgen_anon_1.comp_ })
        })
        .flatten()
        .filter(|c| !c.is_null());
    let compartment: &'static CompartmentCell = match existing {
        // SAFETY: compartment pointers come from CompartmentCell leaks only.
        Some(c) => unsafe { &*(c as *const CompartmentCell) },
        None => {
            let system =
                !principals.is_null() && principals as usize == heap.trusted_principals.get();
            let cell = Box::leak(Box::new(CompartmentCell { context, system }));
            heap.compartments.borrow_mut().push(cell);
            cell
        }
    };
    let created = heap.with_engine(|engine| -> Result<(RealmId, ObjectId), ScriptError> {
        let realm = engine.create_global_realm()?;
        let global = engine.realm_global(realm)?;
        engine.attach_host(global, clasp as u64, 0, 0, true)?;
        Ok((realm, global))
    });
    let Some((realm, global)) = engine_result(context, created) else {
        return ptr::null_mut();
    };
    if !principals.is_null() {
        unsafe { super::JS_HoldPrincipals(principals) };
    }
    let realm_cell: &'static RealmCell = Box::leak(Box::new(RealmCell {
        context,
        id: realm,
        global: Cell::new(ptr::null_mut()),
        principals,
        compartment,
    }));
    heap.realms.borrow_mut().insert(realm, realm_cell);
    heap.init_slots(global, clasp);
    let object = heap.with_engine(|engine| heap.object(context, engine, global, clasp));
    realm_cell.global.set(object);
    object
}

/// The debugger's onNewGlobalObject hook; there is no debugger.
pub unsafe fn JS_FireOnNewGlobalObject(cx: *mut root::JSContext, _global: JS::HandleObject) {
    unsafe { context(cx) };
}

pub unsafe fn EnterRealm(cx: *mut root::JSContext, target: *mut root::JSObject) -> *mut JS::Realm {
    let context = unsafe { context(cx) };
    let cell = unsafe { object_cell(target) };
    let realm = cell.realm();
    let old = context.realm();
    let id = unsafe { realm_cell(realm) }.id;
    let entered = context.heap.with_engine(|engine| engine.enter_realm_id(id));
    engine_result(context, entered).expect("mozjs facade: EnterRealm on an unknown realm");
    context.set_realm(realm);
    old
}

pub unsafe fn LeaveRealm(cx: *mut root::JSContext, old_realm: *mut JS::Realm) {
    let context = unsafe { context(cx) };
    if !old_realm.is_null() {
        let id = unsafe { realm_cell(old_realm) }.id;
        let left = context.heap.with_engine(|engine| engine.enter_realm_id(id));
        engine_result(context, left).expect("mozjs facade: LeaveRealm to an unknown realm");
    }
    context.set_realm(old_realm);
}

pub unsafe fn GetRealmGlobalOrNull(realm: *mut JS::Realm) -> *mut root::JSObject {
    if realm.is_null() {
        return ptr::null_mut();
    }
    unsafe { realm_cell(realm) }.global.get()
}

pub unsafe fn CurrentGlobalOrNull(cx: *mut root::JSContext) -> *mut root::JSObject {
    unsafe { GetRealmGlobalOrNull(context(cx).realm()) }
}

pub unsafe fn GetObjectRealmOrNull(obj: *mut root::JSObject) -> *mut JS::Realm {
    unsafe { object_cell(obj) }.realm()
}

pub unsafe fn GetNonCCWObjectGlobal(obj: *mut root::JSObject) -> *mut root::JSObject {
    unsafe { GetRealmGlobalOrNull(object_cell(obj).realm()) }
}

pub unsafe fn JS_IsGlobalObject(obj: *mut root::JSObject) -> bool {
    unsafe { GetNonCCWObjectGlobal(obj) == obj }
}

pub unsafe fn GetRealmPrincipals(realm: *mut JS::Realm) -> *mut root::JSPrincipals {
    unsafe { realm_cell(realm) }.principals
}

pub unsafe fn JS_SetTrustedPrincipals(cx: *mut root::JSContext, prin: *mut root::JSPrincipals) {
    unsafe { context(cx) }
        .heap
        .trusted_principals
        .set(prin as usize);
}

// --- Compartments ---

pub unsafe fn JS_IterateCompartments(
    cx: *mut root::JSContext,
    data: *mut c_void,
    callback: root::JSIterateCompartmentCallback,
) {
    let context = unsafe { context(cx) };
    let Some(callback) = callback else { return };
    // Copy the list: the callback may create globals.
    let compartments = context.heap.compartments.borrow().clone();
    for compartment in compartments {
        let ptr = compartment as *const CompartmentCell as *mut JS::Compartment;
        // SAFETY: the embedder's callback, with its own data pointer.
        if unsafe { callback(cx, data, ptr) } == JS::CompartmentIterResult::Stop {
            break;
        }
    }
}

/// Every compartment can hold more realms: there are no per-compartment
/// invariants (no wrappers) to protect.
pub unsafe fn IsSharableCompartment(comp: *mut JS::Compartment) -> bool {
    !comp.is_null()
}

pub unsafe fn IsSystemCompartment(comp: *mut JS::Compartment) -> bool {
    assert!(!comp.is_null(), "mozjs facade: null Compartment");
    // SAFETY: compartment pointers come from CompartmentCell leaks only.
    unsafe { &*(comp as *const CompartmentCell) }.system
}

pub unsafe fn JS_GetCompartmentPrivate(_compartment: *mut JS::Compartment) -> *mut c_void {
    ptr::null_mut()
}

// --- Objects and reserved slots (E1) ---

pub unsafe fn JS_NewObjectWithGivenProto(
    cx: *mut root::JSContext,
    clasp: *const root::JSClass,
    proto: JS::Handle<*mut root::JSObject>,
) -> *mut root::JSObject {
    // SAFETY: a Handle points to a rooted, live location.
    let proto = unsafe { *proto.ptr };
    let proto = (!proto.is_null()).then(|| unsafe { object_cell(proto) }.id);
    unsafe { new_object(cx, clasp, proto) }
}

/// The prototype is %Object.prototype% of the current realm (classes with a
/// JSProtoKey come with the standard class entry points).
pub unsafe fn JS_NewObject(
    cx: *mut root::JSContext,
    clasp: *const root::JSClass,
) -> *mut root::JSObject {
    let context = unsafe { context(cx) };
    let proto = context.heap.with_engine(|engine| engine.object_prototype());
    unsafe { new_object(cx, clasp, proto) }
}

pub(super) unsafe fn new_object(
    cx: *mut root::JSContext,
    clasp: *const root::JSClass,
    proto: Option<ObjectId>,
) -> *mut root::JSObject {
    let context = unsafe { context(cx) };
    let heap = &context.heap;
    let created = heap.with_engine(|engine| {
        if clasp.is_null() {
            engine.create_object(proto)
        } else {
            engine.create_host_object(proto, clasp as u64, 0, 0, true)
        }
    });
    let Some(id) = engine_result(context, created) else {
        return ptr::null_mut();
    };
    heap.init_slots(id, clasp);
    heap.with_engine(|engine| heap.object(context, engine, id, clasp))
}

pub unsafe fn JS_GetReservedSlot(obj: *mut root::JSObject, index: u32, dest: *mut JS::Value) {
    let cell = unsafe { object_cell(obj) };
    let value = cell
        .context()
        .heap
        .shared
        .slots
        .borrow()
        .get(&cell.id)
        .and_then(|slots| slots.get(index as usize).copied())
        .expect("mozjs facade: reserved slot out of range");
    // SAFETY: `dest` is the caller's out-parameter.
    unsafe { *dest = value };
}

pub unsafe fn JS_SetReservedSlot(obj: *mut root::JSObject, index: u32, v: *const JS::Value) {
    let cell = unsafe { object_cell(obj) };
    // SAFETY: `v` is the caller's value.
    let value = unsafe { *v };
    let shared = &cell.context().heap.shared;
    let mut slots = shared.slots.borrow_mut();
    let slot = slots
        .get_mut(&cell.id)
        .and_then(|slots| slots.get_mut(index as usize))
        .expect("mozjs facade: reserved slot out of range");
    *slot = value;
}

// --- Collection ---

pub unsafe fn JS_GC(cx: *mut root::JSContext, _reason: JS::GCReason) {
    let context = unsafe { context(cx) };
    if context.heap.engine.borrow().is_some() {
        context.heap.with_engine(|engine| engine.collect());
    }
}
