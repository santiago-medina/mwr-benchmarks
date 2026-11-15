/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Engine, context and runtime lifecycle, plus the configuration and callback
//! registration calls Servo makes while it creates a script runtime
//! (components/script/runtime/script_runtime.rs, `Runtime::new_with_parent`).
//!
//! A `JSContext*` points to a [`Context`]. Its first field is a
//! `JS::RootingContext`, because the Rust layer (`RootedBase`, `jsimpls.rs`)
//! casts the context to that type to link stack roots, exactly as
//! SpiderMonkey's `JS::RootingContext::get` does. One context owns one
//! runtime, so a `JSRuntime*` is the same address as its `JSContext*`.
//!
//! Configuration that has no meaning for this engine (JIT options, GC tuning,
//! stack quotas, the self-hosted code cache) is accepted and recorded so a
//! later read sees what was set; it changes nothing.

use std::cell::{RefCell, UnsafeCell};
use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::sys::glue;
use crate::sys::jsapi as root;
use crate::sys::jsapi::JS;

/// The object behind every `JSContext*` (and `JSRuntime*`) the facade hands out.
#[repr(C)]
pub struct Context {
    /// Must stay first: `JSContext*` is cast to `JS::RootingContext*`.
    /// The Rust layer and EnterRealm/LeaveRealm write through it while
    /// shared references to the context exist, hence the `UnsafeCell`
    /// (`repr(transparent)`, so the layout is the RootingContext's).
    rooting: UnsafeCell<JS::RootingContext>,
    options: JS::ContextOptions,
    pub(crate) state: RefCell<State>,
    /// The engine and the facade's object, realm and compartment cells.
    pub(crate) heap: super::objects::Heap,
}

/// Callbacks and settings registered on a context. Raw pointers are the
/// embedder's closure data, handed back unchanged when the callback runs.
#[derive(Default)]
pub struct State {
    pub gc_params: HashMap<u32, u32>,
    pub stack_quota: [usize; 3],
    pub extra_roots: Vec<(root::JSTraceDataOp, *mut c_void)>,
    pub warning_reporter: JS::WarningReporter,
    pub gc_callback: Option<(root::JSGCCallback, *mut c_void)>,
    pub gc_slice_callback: JS::GCSliceCallback,
    pub dom_callbacks: Option<*const root::js::DOMCallbacks>,
    pub preserve_wrapper: Option<(
        root::js::PreserveWrapperCallback,
        root::js::HasReleasedWrapperCallback,
    )>,
    pub security_callbacks: Option<*const root::JSSecurityCallbacks>,
    pub destroy_principals: root::JSDestroyPrincipalsOp,
    pub read_principals: root::JSReadPrincipalsOp,
    pub consume_stream: Option<(JS::ConsumeStreamCallback, JS::ReportStreamErrorCallback)>,
    pub promise_rejection_tracker: Option<(JS::PromiseRejectionTrackerCallback, *mut c_void)>,
    pub event_loop_dispatch: Option<EventLoopDispatch>,
    pub script_environment_preparer: glue::InvokeScriptPreparerHook,
    pub job_queue: Option<*mut JS::JobQueue>,
    pub module_load_hook: JS::ModuleLoadHook,
    pub module_metadata_hook: JS::ModuleMetadataHook,
    pub script_private_hooks: Option<(
        JS::ScriptPrivateReferenceHook,
        JS::ScriptPrivateReferenceHook,
    )>,
    pub futex_can_wait: bool,
    pub offthread_compilation: bool,
    pub jit_options: HashMap<u32, u32>,
    pub interrupt_callbacks: Vec<root::JSInterruptCallback>,
    /// The exception a failed entry point or JSNative left on the context
    /// (JS_SetPendingException); cells are never freed, so a raw JSVal
    /// stays valid.
    pub pending_exception: Option<crate::sys::jsval::JSVal>,
    /// HideScriptedCaller nesting depth.
    pub hidden_scripted_caller: usize,
    /// JS_ErrorFromException reports, one per error object. Like cells they
    /// are never freed, so a BorrowedErrorReport stays valid.
    pub error_reports: HashMap<gc_lab::script::ObjectId, Box<super::scripts::ErrorReportCell>>,
}

pub struct EventLoopDispatch {
    pub callback: glue::RustDispatchToEventLoopCallback,
    pub started: JS::AsyncTaskStartedCallback,
    pub finished: JS::AsyncTaskFinishedCallback,
    pub closure: *mut c_void,
}

/// The context behind `cx`.
///
/// # Safety
/// `cx` must come from [`JS_NewContext`] and not be destroyed yet. Contexts are
/// single-threaded, as in SpiderMonkey; the `RefCell` in [`State`] catches
/// reentrant mutation instead of allowing it.
pub unsafe fn context<'a>(cx: *mut root::JSContext) -> &'a Context {
    assert!(!cx.is_null(), "mozjs facade: null JSContext");
    // SAFETY: by the contract above, `cx` is a live `Box<Context>` allocation.
    unsafe { &*(cx as *const Context) }
}

impl Context {
    /// The realm the context is in (`JS::RootingContext::realm_`).
    pub fn realm(&self) -> *mut JS::Realm {
        // SAFETY: a plain field read; the context is single-threaded.
        unsafe { (*self.rooting.get()).realm_ }
    }

    pub fn set_realm(&self, realm: *mut JS::Realm) {
        // SAFETY: as above; no reference into the RootingContext is held
        // across this write.
        unsafe { (*self.rooting.get()).realm_ = realm };
    }
}

pub(crate) unsafe fn runtime_context<'a>(rt: *mut root::JSRuntime) -> &'a Context {
    // SAFETY: a runtime pointer is its context pointer (module docs).
    unsafe { context(rt as *mut root::JSContext) }
}

// Process-wide state (SpiderMonkey keeps these in statics too).
static JIT_DISABLED: AtomicBool = AtomicBool::new(false);
static BUILD_ID_OP: Mutex<JS::BuildIdOp> = Mutex::new(None);
static WANT_TO_MEASURE: Mutex<glue::WantToMeasure> = Mutex::new(None);
/// (DOM proxy handler family, shadows check, remote family); pointers as
/// addresses so the tuple is `Send`.
static DOM_PROXY: Mutex<Option<(usize, JS::DOMProxyShadowsCheck, usize)>> = Mutex::new(None);
static LIVE_CONTEXTS: AtomicUsize = AtomicUsize::new(0);

pub fn jit_disabled() -> bool {
    JIT_DISABLED.load(Ordering::Relaxed)
}

pub fn build_id_op() -> JS::BuildIdOp {
    *BUILD_ID_OP.lock().unwrap()
}

pub fn want_to_measure() -> glue::WantToMeasure {
    *WANT_TO_MEASURE.lock().unwrap()
}

pub fn dom_proxy_information() -> Option<(*const c_void, JS::DOMProxyShadowsCheck, *const c_void)> {
    DOM_PROXY
        .lock()
        .unwrap()
        .map(|(family, check, remote)| (family as *const c_void, check, remote as *const c_void))
}

/// Number of contexts created and not destroyed yet.
pub fn live_contexts() -> usize {
    LIVE_CONTEXTS.load(Ordering::SeqCst)
}

// --- Engine lifecycle (js/public/Initialization.h) ---

pub unsafe fn JS_Init() -> bool {
    true
}

pub unsafe fn JS_ShutDown() {}

pub unsafe fn DisableJitBackend() {
    JIT_DISABLED.store(true, Ordering::Relaxed);
}

pub unsafe fn InitializeMemoryReporter(wtm: glue::WantToMeasure) {
    *WANT_TO_MEASURE.lock().unwrap() = wtm;
}

pub unsafe fn SetProcessBuildIdOp(build_id_op: JS::BuildIdOp) {
    *BUILD_ID_OP.lock().unwrap() = build_id_op;
}

pub unsafe fn SetDOMProxyInformation(
    family: *const c_void,
    shadows_check: JS::DOMProxyShadowsCheck,
    remote_family: *const c_void,
) {
    *DOM_PROXY.lock().unwrap() = Some((family as usize, shadows_check, remote_family as usize));
}

// --- Context and runtime (js/public/Context.h) ---

pub unsafe fn JS_NewContext(_maxbytes: u32, _parent: *mut root::JSRuntime) -> *mut root::JSContext {
    // A child runtime (a worker) gets its own engine and heap here; values
    // cross between them only by structured clone, so the parent is unused.
    let context = Box::new(Context {
        // SAFETY: RootingContext is plain data (list heads, profiler and
        // nursery pointers, stack limits); all zero is SpiderMonkey's empty
        // state: no rooted values, no realm entered, no stack limit.
        rooting: UnsafeCell::new(unsafe { std::mem::zeroed() }),
        // SAFETY: ContextOptions is bitfields plus plain compile options.
        options: unsafe { std::mem::zeroed() },
        state: RefCell::new(State::default()),
        heap: super::objects::Heap::default(),
    });
    context
        .heap
        .shared
        .context
        .set(&*context as *const Context as usize);
    LIVE_CONTEXTS.fetch_add(1, Ordering::SeqCst);
    Box::into_raw(context) as *mut root::JSContext
}

pub unsafe fn JS_DestroyContext(cx: *mut root::JSContext) {
    assert!(!cx.is_null(), "mozjs facade: null JSContext");
    // SAFETY: `cx` came from Box::into_raw in JS_NewContext and is destroyed once.
    drop(unsafe { Box::from_raw(cx as *mut Context) });
    LIVE_CONTEXTS.fetch_sub(1, Ordering::SeqCst);
}

pub unsafe fn JS_GetRuntime(cx: *mut root::JSContext) -> *mut root::JSRuntime {
    unsafe { context(cx) };
    cx as *mut root::JSRuntime
}

pub unsafe fn JS_GetParentRuntime(cx: *mut root::JSContext) -> *mut root::JSRuntime {
    unsafe { JS_GetRuntime(cx) }
}

pub unsafe fn ContextOptionsRef(cx: *mut root::JSContext) -> *mut JS::ContextOptions {
    let context = unsafe { context(cx) };
    &context.options as *const JS::ContextOptions as *mut JS::ContextOptions
}

pub unsafe fn JS_SetNativeStackQuota(
    cx: *mut root::JSContext,
    system: JS::NativeStackSize,
    trusted: JS::NativeStackSize,
    untrusted: JS::NativeStackSize,
) {
    unsafe { context(cx) }.state.borrow_mut().stack_quota = [system, trusted, untrusted];
}

pub unsafe fn InitSelfHostedCode(
    cx: *mut root::JSContext,
    _cache: JS::SelfHostedCache,
    _writer: JS::SelfHostedWriter,
) -> bool {
    // The engine's built-ins are native; there is no self-hosted JS to load.
    unsafe { context(cx) };
    true
}

pub unsafe fn SetWarningReporter(
    cx: *mut root::JSContext,
    reporter: JS::WarningReporter,
) -> JS::WarningReporter {
    std::mem::replace(
        &mut unsafe { context(cx) }.state.borrow_mut().warning_reporter,
        reporter,
    )
}

pub unsafe fn JS_SetFutexCanWait(cx: *mut root::JSContext) {
    unsafe { context(cx) }.state.borrow_mut().futex_can_wait = true;
}

// --- GC configuration and callbacks (js/public/GCAPI.h) ---

pub unsafe fn JS_SetGCParameter(cx: *mut root::JSContext, key: root::JSGCParamKey, value: u32) {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .gc_params
        .insert(key as u32, value);
}

pub unsafe fn JS_GetGCParameter(cx: *mut root::JSContext, key: root::JSGCParamKey) -> u32 {
    let state = unsafe { context(cx) }.state.borrow();
    state.gc_params.get(&(key as u32)).copied().unwrap_or(0)
}

pub unsafe fn JS_AddExtraGCRootsTracer(
    cx: *mut root::JSContext,
    op: root::JSTraceDataOp,
    data: *mut c_void,
) -> bool {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .extra_roots
        .push((op, data));
    true
}

pub unsafe fn JS_RemoveExtraGCRootsTracer(
    cx: *mut root::JSContext,
    op: root::JSTraceDataOp,
    data: *mut c_void,
) {
    let mut state = unsafe { context(cx) }.state.borrow_mut();
    // Same match as SpiderMonkey: the (op, data) pair, first occurrence.
    if let Some(i) = state
        .extra_roots
        .iter()
        .position(|(o, d)| o.map(|f| f as usize) == op.map(|f| f as usize) && *d == data)
    {
        state.extra_roots.remove(i);
    }
}

pub unsafe fn JS_SetGCCallback(
    cx: *mut root::JSContext,
    cb: root::JSGCCallback,
    data: *mut c_void,
) {
    unsafe { context(cx) }.state.borrow_mut().gc_callback = cb.map(|_| (cb, data));
}

pub unsafe fn SetGCSliceCallback(
    cx: *mut root::JSContext,
    callback: JS::GCSliceCallback,
) -> JS::GCSliceCallback {
    std::mem::replace(
        &mut unsafe { context(cx) }.state.borrow_mut().gc_slice_callback,
        callback,
    )
}

pub unsafe fn SetDOMCallbacks(cx: *mut root::JSContext, callbacks: *const root::js::DOMCallbacks) {
    unsafe { context(cx) }.state.borrow_mut().dom_callbacks =
        (!callbacks.is_null()).then_some(callbacks);
}

pub unsafe fn SetPreserveWrapperCallbacks(
    cx: *mut root::JSContext,
    preserve: root::js::PreserveWrapperCallback,
    has_released: root::js::HasReleasedWrapperCallback,
) {
    unsafe { context(cx) }.state.borrow_mut().preserve_wrapper = Some((preserve, has_released));
}

// --- Security and principals (jsapi.h) ---

pub unsafe fn JS_SetSecurityCallbacks(
    cx: *mut root::JSContext,
    callbacks: *const root::JSSecurityCallbacks,
) {
    unsafe { context(cx) }.state.borrow_mut().security_callbacks =
        (!callbacks.is_null()).then_some(callbacks);
}

pub unsafe fn JS_InitDestroyPrincipalsCallback(
    cx: *mut root::JSContext,
    op: root::JSDestroyPrincipalsOp,
) {
    unsafe { context(cx) }.state.borrow_mut().destroy_principals = op;
}

pub unsafe fn JS_InitReadPrincipalsCallback(
    cx: *mut root::JSContext,
    op: root::JSReadPrincipalsOp,
) {
    unsafe { context(cx) }.state.borrow_mut().read_principals = op;
}

// --- JIT options: accepted and recorded; the engine has no JIT ---

pub unsafe fn JS_SetGlobalJitCompilerOption(
    cx: *mut root::JSContext,
    opt: root::JSJitCompilerOption,
    value: u32,
) {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .jit_options
        .insert(opt as u32, value);
}

pub unsafe fn JS_SetOffthreadIonCompilationEnabled(cx: *mut root::JSContext, enabled: bool) {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .offthread_compilation = enabled;
}

// --- Host hooks: promises, jobs, streams, modules, event loop ---

pub unsafe fn InitConsumeStreamCallback(
    cx: *mut root::JSContext,
    consume: JS::ConsumeStreamCallback,
    report: JS::ReportStreamErrorCallback,
) {
    unsafe { context(cx) }.state.borrow_mut().consume_stream = Some((consume, report));
}

pub unsafe fn SetPromiseRejectionTrackerCallback(
    cx: *mut root::JSContext,
    callback: JS::PromiseRejectionTrackerCallback,
    data: *mut c_void,
) {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .promise_rejection_tracker = Some((callback, data));
}

pub unsafe fn SetUpEventLoopDispatch(
    cx: *mut root::JSContext,
    callback: glue::RustDispatchToEventLoopCallback,
    started: JS::AsyncTaskStartedCallback,
    finished: JS::AsyncTaskFinishedCallback,
    closure: *mut c_void,
) {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .event_loop_dispatch = Some(EventLoopDispatch {
        callback,
        started,
        finished,
        closure,
    });
}

pub unsafe fn RegisterScriptEnvironmentPreparer(
    cx: *mut root::JSContext,
    hook: glue::InvokeScriptPreparerHook,
) {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .script_environment_preparer = hook;
}

/// A `RustJobQueue` holds the embedder's traps; the engine calls them
/// directly (there is no C++ vtable).
pub unsafe fn CreateJobQueue(traps: *const glue::JobQueueTraps) -> *mut glue::RustJobQueue {
    assert!(!traps.is_null(), "mozjs facade: null JobQueueTraps");
    let queue = Box::new(glue::RustJobQueue {
        vtable_: std::ptr::null(),
        // SAFETY: `traps` is non-null and points to the embedder's traps.
        mTraps: unsafe { std::ptr::read(traps) },
        draining: false,
    });
    Box::into_raw(queue)
}

pub unsafe fn DeleteJobQueue(queue: *mut glue::RustJobQueue) {
    if !queue.is_null() {
        // SAFETY: `queue` came from Box::into_raw in CreateJobQueue.
        drop(unsafe { Box::from_raw(queue) });
    }
}

pub unsafe fn SetJobQueue(cx: *mut root::JSContext, queue: *mut JS::JobQueue) {
    unsafe { context(cx) }.state.borrow_mut().job_queue = (!queue.is_null()).then_some(queue);
}

pub unsafe fn SetModuleLoadHook(rt: *mut root::JSRuntime, func: JS::ModuleLoadHook) {
    unsafe { runtime_context(rt) }
        .state
        .borrow_mut()
        .module_load_hook = func;
}

pub unsafe fn SetModuleMetadataHook(rt: *mut root::JSRuntime, func: JS::ModuleMetadataHook) {
    unsafe { runtime_context(rt) }
        .state
        .borrow_mut()
        .module_metadata_hook = func;
}

pub unsafe fn SetScriptPrivateReferenceHooks(
    rt: *mut root::JSRuntime,
    add_ref: JS::ScriptPrivateReferenceHook,
    release: JS::ScriptPrivateReferenceHook,
) {
    unsafe { runtime_context(rt) }
        .state
        .borrow_mut()
        .script_private_hooks = Some((add_ref, release));
}

// --- Realm options (glue): plain data until realms exist (next step) ---

pub unsafe fn JS_NewRealmOptions() -> *mut JS::RealmOptions {
    // SAFETY: RealmOptions is plain data (option bits, pointers, a locale
    // string pointer); all zero is a valid default for this engine.
    Box::into_raw(Box::new(unsafe { std::mem::zeroed::<JS::RealmOptions>() }))
}

pub unsafe fn DeleteRealmOptions(options: *mut JS::RealmOptions) {
    if !options.is_null() {
        // SAFETY: `options` came from Box::into_raw in JS_NewRealmOptions.
        drop(unsafe { Box::from_raw(options) });
    }
}

pub unsafe fn GetModuleLoadHook(rt: *mut root::JSRuntime) -> JS::ModuleLoadHook {
    unsafe { runtime_context(rt) }
        .state
        .borrow()
        .module_load_hook
}

pub unsafe fn GetModuleMetadataHook(rt: *mut root::JSRuntime) -> JS::ModuleMetadataHook {
    unsafe { runtime_context(rt) }
        .state
        .borrow()
        .module_metadata_hook
}

/// Recorded; the engine does not poll interrupt callbacks yet (they need the
/// step hook that replaces the test step limit, MOZJS_FACADE.md "Next").
pub unsafe fn JS_AddInterruptCallback(
    cx: *mut root::JSContext,
    callback: root::JSInterruptCallback,
) -> bool {
    unsafe { context(cx) }
        .state
        .borrow_mut()
        .interrupt_callbacks
        .push(callback);
    true
}

// js/public/Proxy.h: the WindowProxy class is process-wide in SpiderMonkey.
static WINDOW_PROXY_CLASS: Mutex<usize> = Mutex::new(0);

pub unsafe fn SetWindowProxyClass(cx: *mut root::JSContext, clasp: *const root::JSClass) {
    unsafe { context(cx) };
    *WINDOW_PROXY_CLASS.lock().unwrap() = clasp as usize;
}

pub unsafe fn GetWindowProxyClass() -> *const root::JSClass {
    *WINDOW_PROXY_CLASS.lock().unwrap() as *const root::JSClass
}

// --- Pending exceptions (jsapi.h, js/public/Exception.h) ---

pub unsafe fn JS_IsExceptionPending(cx: *mut root::JSContext) -> bool {
    let cx = unsafe { context(cx) };
    cx.state.borrow().pending_exception.is_some()
}

pub unsafe fn JS_GetPendingException(
    cx: *mut root::JSContext,
    vp: root::JS::MutableHandleValue,
) -> bool {
    let cx = unsafe { context(cx) };
    let Some(value) = cx.state.borrow().pending_exception else {
        return false;
    };
    // SAFETY: the caller's out-parameter.
    unsafe { *vp.ptr = value };
    true
}

pub unsafe fn JS_SetPendingException(
    cx: *mut root::JSContext,
    v: root::JS::HandleValue,
    _behavior: root::JS::ExceptionStackBehavior,
) {
    let cx = unsafe { context(cx) };
    // SAFETY: a rooted handle to a live value.
    cx.state.borrow_mut().pending_exception = Some(unsafe { *v.ptr });
}

pub unsafe fn JS_ClearPendingException(cx: *mut root::JSContext) {
    let cx = unsafe { context(cx) };
    cx.state.borrow_mut().pending_exception = None;
}
