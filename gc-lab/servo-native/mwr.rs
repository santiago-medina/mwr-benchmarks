//! Opt-in experiment. Our AST and closures run on Servo's script thread.
//! SpiderMonkey still owns Servo's DOM reflectors; this is not a replacement VM.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use crate::runtime::job_queue::MicrotaskRunnable;
use crate::{DomTypeHolder, JSTraceable};
use gc_lab::script::{DomAdapter, Engine, SuspendedEngine};
use gc_lab::Model;
use js::context::JSContext;
use js::jsapi::JSTracer;
use script_bindings::settings_stack::run_a_script;
use servo_base::id::PipelineId;

use crate::dom::bindings::codegen::Bindings::DocumentBinding::DocumentMethods;
use crate::dom::bindings::codegen::Bindings::ElementBinding::ElementMethods;
use crate::dom::bindings::codegen::Bindings::HTMLElementBinding::HTMLElementMethods;
use crate::dom::bindings::codegen::Bindings::NodeBinding::NodeMethods;
use crate::dom::bindings::codegen::UnionTypes::StringOrElementCreationOptions;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::DOMString;
use crate::dom::document::Document;
use crate::dom::element::Element;
use crate::dom::eventtarget::EventTarget;
use crate::dom::html::htmlelement::HTMLElement;
use crate::dom::node::Node;
use crate::dom::window::Window;

#[path = "mwr_gc_test.rs"]
mod gc_test;
#[path = "mwr_modules.rs"]
mod module_loader;
#[path = "mwr_network.rs"]
mod network;

type Result<T> = std::result::Result<T, String>;
thread_local! {
    static SESSIONS: RefCell<HashMap<PipelineId, Rc<Session>>> = RefCell::new(HashMap::new());
}
struct Session {
    module_load: RefCell<module_loader::Load>,
    engine: RefCell<Option<SuspendedEngine>>,
    registry: Rc<RefCell<Registry>>,
    marked: RefCell<HashSet<u64>>,
    collection_queued: Cell<bool>,
    stress: RefCell<Option<gc_test::Progress>>,
    callbacks: Cell<usize>,
    lifetime: Life,
}
struct Registry {
    network: network::Network,
    document: WeakNode,
    nodes: HashMap<u64, WeakNode>,
    pins: Vec<Trusted<Node>>,
    next: u64,
}
impl Registry {
    fn intern(&mut self, node: &Node) -> Result<u64> {
        for (id, trusted) in &self.nodes {
            if trusted.root().is_some_and(|n| &*n == node) {
                return Ok(*id);
            }
        }
        let id = self.next;
        self.next = id.checked_add(1).ok_or("native ID space exhausted")?;
        self.nodes.insert(id, WeakNode::new(node));
        self.pins.push(Trusted::new(node));
        Ok(id)
    }
    fn node(&self, id: u64) -> Result<DomRoot<Node>> {
        self.nodes
            .get(&id)
            .and_then(WeakNode::root)
            .ok_or("released native node".into())
    }
}
// Borrow the ONE current context, only for this invocation. Never manufacture a
// second JSContext or hold the context across event-loop turns.
// This token does not own the node. Servo's native finalizer drops its last
// strong token; a dead handle is then rejected BEFORE touching the Dom pointer.
// Rust DOM allocations do not move; tracing their reflectors updates JS pointers.
// Cross-thread test observers receive only the atomic death flag, never a DOM
// pointer or an Rc. Keeping the flag alive does not retain its native owner.
#[derive(Default)]
pub(crate) struct Life {
    dead: Arc<AtomicBool>,
}
impl Drop for Life {
    fn drop(&mut self) {
        self.dead.store(true, Ordering::Release);
    }
}
#[expect(unsafe_code)]
unsafe impl JSTraceable for Life {
    // Life contains only an atomic flag; it cannot hide any managed edge.
    unsafe fn trace(&self, _: *mut JSTracer) {}
}

#[derive(Clone)]
struct WeakNode {
    node: Dom<Node>,
    alive: Weak<Life>,
}
impl WeakNode {
    fn new(node: &Node) -> Self {
        Self {
            node: Dom::from_ref(node),
            alive: node.mwr_lifetime(),
        }
    }
    fn root(&self) -> Option<DomRoot<Node>> {
        // Same script thread. No GC-capable call occurs between the liveness
        // check and installing this stack root. Never use the bare Dom pointer.
        self.alive.upgrade()?;
        Some(self.node.as_rooted())
    }
    #[expect(unsafe_code)]
    unsafe fn trace(&self, tracer: *mut JSTracer) {
        // Mark phase only; a live token means the native finalizer has not run.
        if self.alive.upgrade().is_some() {
            unsafe {
                self.node.trace(tracer);
            }
        }
    }
}
impl Registry {
    fn document(&self) -> Result<DomRoot<Document>> {
        let node = self.document.root().ok_or("document finalized")?;
        node.downcast::<Document>()
            .map(DomRoot::from_ref)
            .ok_or("not a document".into())
    }
}

#[derive(Clone, PartialEq, MallocSizeOf)]
pub(crate) struct Listener {
    pipeline: PipelineId,
    id: u64,
}
impl Listener {
    pub(crate) fn new(target: &EventTarget, id: u64) -> Self {
        Self {
            pipeline: target.global().pipeline_id(),
            id,
        }
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
}
#[expect(unsafe_code)]
unsafe impl JSTraceable for Listener {
    unsafe fn trace(&self, tracer: *mut JSTracer) {
        if let Some(state) = SESSIONS.with(|s| s.borrow().get(&self.pipeline).cloned()) {
            unsafe {
                trace_state(tracer, &state, Some(self.id));
            }
        }
    }
}
#[expect(unsafe_code)]
unsafe fn trace_state(tracer: *mut JSTracer, state: &Session, listener: Option<u64>) {
    let remotes = match state.engine.borrow().as_ref() {
        Some(engine) => engine.trace_native_edges(listener),
        // Active host invocations temporarily pin their native objects; VM
        // frames/local foreign pins protect the other half until it suspends.
        None => return,
    };
    state.marked.borrow_mut().extend(remotes.iter().copied());
    let nodes: Vec<_> = {
        let registry = state.registry.borrow();
        remotes
            .iter()
            .filter_map(|id| registry.nodes.get(id).cloned())
            .collect()
    };
    // Do not keep a RefCell borrowed across a call to the native tracer.
    for node in nodes {
        unsafe {
            node.trace(tracer);
        }
    }
}
#[expect(unsafe_code)]
pub(crate) unsafe fn trace_roots(tracer: *mut JSTracer) {
    let states: Vec<_> = SESSIONS.with(|s| s.borrow().values().cloned().collect());
    for state in states {
        unsafe {
            trace_state(tracer, &state, None);
        }
    }
}

struct NativeAdapter<'cx> {
    cx: &'cx mut JSContext,
    registry: Rc<RefCell<Registry>>,
}
impl<'cx> NativeAdapter<'cx> {
    fn new(cx: &'cx mut JSContext, registry: Rc<RefCell<Registry>>) -> Self {
        {
            let mut r = registry.borrow_mut();
            assert!(r.pins.is_empty(), "nested native adapter");
            r.pins = r
                .nodes
                .values()
                .filter_map(WeakNode::root)
                .map(|node| Trusted::new(&*node))
                .collect();
        }
        Self { cx, registry }
    }
}
impl Drop for NativeAdapter<'_> {
    fn drop(&mut self) {
        self.registry.borrow_mut().pins.clear();
    }
}
impl DomAdapter for NativeAdapter<'_> {
    fn supports_fetch(&self) -> bool {
        true
    }
    fn start_fetch(&mut self, serial: u64, url: &str) -> Result<()> {
        self.start_network(serial, url)
    }
    fn read_response_text(&mut self, serial: u64, response: u64) -> Result<()> {
        self.read_network_text(serial, response)
    }
    fn cancel_fetch(&mut self, serial: u64) {
        self.cancel_network(serial);
    }
    fn release_responses(&mut self, responses: &[u64]) -> Result<()> {
        self.release_network(responses);
        Ok(())
    }

    fn queue_microtask(&mut self, serial: u64) -> Result<()> {
        let doc = self.registry.borrow().document()?;
        doc.global().enqueue_microtask(
            self.cx,
            Box::new(MwrMicrotask {
                pipeline: doc.window().pipeline_id(),
                serial,
            }),
        );
        Ok(())
    }
    fn traces_native_edges(&self) -> bool {
        true
    }

    fn create_element(&mut self, tag: &str) -> Result<u64> {
        let doc = self.registry.borrow().document()?;
        let element = doc
            .CreateElement(
                self.cx,
                DOMString::from(tag),
                StringOrElementCreationOptions::String(DOMString::new()),
            )
            .map_err(|e| format!("CreateElement: {e:?}"))?;
        self.registry.borrow_mut().intern(element.upcast::<Node>())
    }
    fn query_selector(&mut self, selector: &str) -> Result<u64> {
        let doc = self.registry.borrow().document()?;
        let element = doc
            .QuerySelector(self.cx, DOMString::from(selector))
            .map_err(|e| format!("QuerySelector: {e:?}"))?
            .ok_or("selector did not match")?;
        self.registry.borrow_mut().intern(element.upcast::<Node>())
    }
    fn tag_name(&mut self, id: u64) -> Result<String> {
        let node = self.registry.borrow().node(id)?;
        Ok(node
            .downcast::<Element>()
            .ok_or("not an element")?
            .TagName()
            .to_string())
    }
    fn text_content(&mut self, id: u64) -> Result<String> {
        let node = self.registry.borrow().node(id)?;
        Ok(node.GetTextContent().unwrap_or_default().to_string())
    }
    fn set_text_content(&mut self, id: u64, text: &str) -> Result<()> {
        let node = self.registry.borrow().node(id)?;
        node.SetTextContent(self.cx, Some(DOMString::from(text)))
            .map_err(|e| format!("SetTextContent: {e:?}"))
    }
    fn append_child(&mut self, parent: u64, child: u64) -> Result<()> {
        let parent = self.registry.borrow().node(parent)?;
        let child = self.registry.borrow().node(child)?;
        parent
            .AppendChild(self.cx, &child)
            .map(|_| ())
            .map_err(|e| format!("AppendChild: {e:?}"))
    }
    fn remove_child(&mut self, parent: u64, child: u64) -> Result<()> {
        let parent = self.registry.borrow().node(parent)?;
        let child = self.registry.borrow().node(child)?;
        parent
            .RemoveChild(self.cx, &child)
            .map(|_| ())
            .map_err(|e| format!("RemoveChild: {e:?}"))
    }
    fn set_click_listener(&mut self, id: u64, enabled: bool) -> Result<()> {
        let node = self.registry.borrow().node(id)?;
        node.upcast::<EventTarget>().set_mwr_listener(id, enabled);
        Ok(())
    }
    fn click(&mut self, _: u64) -> Result<Vec<u64>> {
        // Native dispatch can synchronously reenter arbitrary DOM callbacks.
        // Do not promise programmatic reentry until that boundary is implemented.
        Err("programmatic click is not supported by the native prototype".into())
    }
    fn release(&mut self, ids: &[u64]) -> Result<()> {
        for id in ids {
            let node = self.registry.borrow_mut().nodes.remove(id);
            if let Some(node) = node {
                if let Some(node) = node.root() {
                    node.upcast::<EventTarget>().set_mwr_listener(*id, false);
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn enabled() -> bool {
    std::env::var_os("MWR_NATIVE_JS").is_some_and(|s| s == "1")
}
fn session(window: &Window) -> Option<Rc<Session>> {
    SESSIONS.with(|s| s.borrow().get(&window.pipeline_id()).cloned())
}
pub(crate) fn execute(cx: &mut JSContext, doc: &Document, source: &str) {
    let global = doc.global();
    run_a_script::<DomTypeHolder, _, _>(cx, &global, |cx| {
        if let Err(error) = execute_inner(cx, doc, source) {
            eprintln!("MWR_NATIVE_ERROR {error}");
            clear(doc.window());
        }
    });
}
fn create_session(cx: &mut JSContext, doc: &Document, source: &str) -> Result<Rc<Session>> {
    let window = doc.window();
    if session(window).is_some() {
        return Err("only one experimental script per document is supported".into());
    }
    let state = Rc::new(Session {
        module_load: RefCell::new(module_loader::Load::default()),
        engine: RefCell::new(None),
        registry: Rc::new(RefCell::new(Registry {
            network: network::Network::default(),
            document: WeakNode::new(doc.upcast::<Node>()),
            pins: vec![],
            nodes: HashMap::new(),
            next: 1,
        })),
        marked: RefCell::new(HashSet::new()),
        collection_queued: Cell::new(false),
        stress: RefCell::new(None),
        callbacks: Cell::new(0),
        lifetime: Life::default(),
    });
    let mut engine = Engine::new(Model::Indirect, Model::Indirect).map_err(|e| format!("{e:?}"))?;
    engine
        .set_dom_adapter(Box::new(NativeAdapter::new(cx, state.registry.clone())))
        .map_err(|e| format!("{e:?}"))?;
    engine.execute(source).map_err(|e| format!("{e:?}"))?;
    *state.engine.borrow_mut() = Some(engine.suspend());
    SESSIONS.with(|s| s.borrow_mut().insert(window.pipeline_id(), state.clone()));
    Ok(state)
}
fn execute_inner(cx: &mut JSContext, doc: &Document, source: &str) -> Result<()> {
    create_session(cx, doc, source)?;
    eprintln!("MWR_NATIVE_EXECUTED");
    network::on_execute(doc);
    queue_collection(doc);
    if std::env::var_os("MWR_NATIVE_GC_TEST").is_some_and(|s| s == "1") {
        gc_test::start(doc);
    }
    if std::env::var_os("MWR_NATIVE_SELF_TEST").is_some_and(|s| s == "1") {
        let document = Trusted::new(doc);
        doc.global()
            .task_manager()
            .dom_manipulation_task_source()
            .queue(task!(MwrNativeSelfTest: move |cx| {
                let document = document.root();
                let mut realm = crate::realms::enter_auto_realm(cx, document.window());
                eprintln!("MWR_NATIVE_TEST_TASK");
                if let Err(error) = self_test(&mut realm.current_realm(), &document) {
                    eprintln!("MWR_NATIVE_ERROR {error}");
                    clear(document.window());
                }
            }));
    }
    Ok(())
}
pub(crate) fn dispatch(cx: &mut JSContext, target: &EventTarget, id: u64) -> Result<()> {
    let global = target.global();
    run_a_script::<DomTypeHolder, _, _>(cx, &global, |cx| dispatch_inner(cx, target, id))
}
fn dispatch_inner(cx: &mut JSContext, target: &EventTarget, id: u64) -> Result<()> {
    let global = target.global();
    let window = global.downcast::<Window>().ok_or("not a window")?;
    let state = session(window).ok_or("no native session")?;
    let dormant = state
        .engine
        .borrow_mut()
        .take()
        .ok_or("native callback reentry is not supported")?;
    let mut engine = dormant.resume(Box::new(NativeAdapter::new(cx, state.registry.clone())));
    let result = engine
        .dispatch_remote_click(id)
        .map_err(|e| format!("{e:?}"));
    *state.engine.borrow_mut() = Some(engine.suspend());
    // A failed callback must also reach a coordinated GC safepoint.
    let doc = state.registry.borrow().document()?;
    queue_collection(&doc);
    result?;
    state.callbacks.set(state.callbacks.get() + 1);
    eprintln!("MWR_NATIVE_CALLBACK");
    Ok(())
}
// This exercises Servo's real DOM event dispatcher in Rust, not a hardware input
// event. Ordinary human clicks use the same foreign-listener branch.
fn self_test(cx: &mut JSContext, doc: &Document) -> Result<()> {
    let selector = std::env::var("MWR_NATIVE_TEST_TARGET").unwrap_or_else(|_| "#button".into());
    let target = doc
        .QuerySelector(cx, DOMString::from(selector))
        .map_err(|e| format!("{e:?}"))?
        .ok_or("test target missing")?;
    let node = target.upcast::<Node>();
    let before = node.GetTextContent().unwrap_or_default().to_string();
    if before != std::env::var("MWR_NATIVE_EXPECT_BEFORE").unwrap_or_else(|_| "Pulsar".into()) {
        return Err(format!("unexpected initial text: {before}"));
    }
    let configured = std::env::var("MWR_NATIVE_TEST_STEPS").ok();
    let steps: Vec<(String, String)> = if let Some(json) = &configured {
        serde_json::from_str(json).map_err(|e| format!("test steps: {e}"))?
    } else {
        vec![(
            "#button".into(),
            std::env::var("MWR_NATIVE_EXPECT_AFTER").unwrap_or_else(|_| "Funciona".into()),
        )]
    };
    let mut observed = vec![];
    for (selector, expected) in steps {
        let button = doc
            .QuerySelector(cx, DOMString::from(selector.clone()))
            .map_err(|e| format!("{e:?}"))?
            .ok_or("test control missing")?;
        button
            .downcast::<HTMLElement>()
            .ok_or("test control not HTML")?
            .Click(cx);
        let actual = node.GetTextContent().unwrap_or_default().to_string();
        if actual != expected {
            return Err(format!(
                "native callback failed: {actual}, expected {expected}"
            ));
        }
        observed.push((selector, actual));
    }
    let after = node.GetTextContent().unwrap_or_default().to_string();
    let mut proof = serde_json::json!({"before":before,"after":after,"click":"rust-dom-synthetic"});
    if configured.is_some() {
        proof["steps"] = serde_json::json!(observed);
    }
    eprintln!("MWR_NATIVE_PROOF {proof}");
    Ok(())
}
// Trusted DOM roots must go before Servo tears down its JS runtime. This hook is
// mandatory on navigation/shutdown; the registry is not a joint tracing GC.
pub(crate) fn abort_network(cx: &mut JSContext, window: &Window) {
    network::abort_all(cx, window);
}
pub(crate) fn retire(cx: &mut JSContext, window: &Window) {
    let state = SESSIONS.with(|s| s.borrow_mut().remove(&window.pipeline_id()));
    if let Some(state) = state {
        // Invalidate callbacks before aborting: native rejection delivery can
        // enqueue work, but an unloaded document must never resume its VM.
        module_loader::abort_state(&state);
        network::abort_state(cx, &state);
        clear_listeners(&state);
    }
}
pub(crate) fn clear(window: &Window) {
    if let Some(state) = SESSIONS.with(|s| s.borrow_mut().remove(&window.pipeline_id())) {
        clear_listeners(&state);
    }
}
fn clear_listeners(state: &Session) {
    for (id, node) in state.registry.borrow_mut().nodes.drain() {
        if let Some(node) = node.root() {
            node.upcast::<EventTarget>().set_mwr_listener(id, false);
        }
    }
}

// Panic teardown must only drop owned roots: DOM methods may no longer be safe.
pub(crate) fn discard(window: &Window) {
    SESSIONS.with(|s| {
        s.borrow_mut().remove(&window.pipeline_id());
    });
}

fn queue_collection(doc: &Document) {
    let Some(state) = session(doc.window()) else {
        return;
    };
    if state.collection_queued.replace(true) {
        return;
    }
    let document = Trusted::new(doc);
    doc.global()
        .task_manager()
        .dom_manipulation_task_source()
        .queue(task!(MwrJointCollection: move |cx| {
            let document = document.root();
            if let Some(state) = session(document.window()) {
                state.collection_queued.set(false);
                let mut realm = crate::realms::enter_auto_realm(cx, document.window());
                if let Err(error) = collect_joint(&mut realm.current_realm(), document.window()) {
                    eprintln!("MWR_NATIVE_ERROR {error}");
                }
            }
        }));
}
// JS_GC is a full non-incremental mark/sweep. The native worklist reaches foreign
// listeners, whose hooks traverse our VM and enqueue further native reflectors.
// Only after that worklist is exhausted may the VM discard conservative pins.
fn collect_joint(cx: &mut JSContext, window: &Window) -> Result<()> {
    let states: Vec<_> = SESSIONS.with(|s| s.borrow().values().cloned().collect());
    for state in &states {
        if state.engine.borrow().is_none() {
            return Err("joint collection during active VM call".into());
        }
        if !state.registry.borrow().pins.is_empty() {
            return Err("native pins escaped an invocation".into());
        }
        state.marked.borrow_mut().clear();
    }
    window.gc(cx);
    for state in states {
        let marked: Vec<_> = state.marked.borrow().iter().copied().collect();
        let dormant = state
            .engine
            .borrow_mut()
            .take()
            .ok_or("VM busy after native collection")?;
        let mut engine = dormant.resume(Box::new(NativeAdapter::new(cx, state.registry.clone())));
        let result = engine.collect_native(&marked).map_err(|e| format!("{e:?}"));
        let host_error = engine.host_error().map(str::to_owned);
        *state.engine.borrow_mut() = Some(engine.suspend());
        result?;
        if let Some(error) = host_error {
            return Err(error);
        }
    }
    Ok(())
}

// Host queue entries hold only stable numeric IDs, never DOM roots or VM pointers.
thread_local! {static MWR_MICROTASKS:Cell<usize>=const{Cell::new(0)};}
static STALE_MICROTASKS: AtomicUsize = AtomicUsize::new(0);
fn stale_microtask_count() -> usize {
    STALE_MICROTASKS.load(Ordering::Acquire)
}
pub(crate) fn begin_microtask_checkpoint() {
    MWR_MICROTASKS.with(|n| n.set(0));
}
#[derive(MallocSizeOf)]
struct MwrMicrotask {
    pipeline: PipelineId,
    serial: u64,
}
#[expect(unsafe_code)]
unsafe impl JSTraceable for MwrMicrotask {
    // Both fields are plain identifiers. Managed edges live in the rooted VM job.
    unsafe fn trace(&self, _: *mut JSTracer) {}
}
impl MicrotaskRunnable for MwrMicrotask {
    fn handler(&self, cx: &mut JSContext) {
        let Some(state) = SESSIONS.with(|s| s.borrow().get(&self.pipeline).cloned()) else {
            STALE_MICROTASKS.fetch_add(1, Ordering::AcqRel);
            eprintln!("MWR_NATIVE_STALE_MICROTASK");
            return;
        };
        let count = MWR_MICROTASKS.with(|n| {
            let count = n.get() + 1;
            n.set(count);
            count
        });
        if count > 10_000 {
            if let Some(engine) = state.engine.borrow_mut().as_mut() {
                engine.cancel_microtasks();
            }
            if count == 10_001 {
                eprintln!("MWR_NATIVE_ERROR microtask checkpoint limit");
            }
            return;
        }
        let doc = match state.registry.borrow().document() {
            Ok(doc) => doc,
            Err(_) => return,
        };
        if std::env::var_os("MWR_NATIVE_MICROTASK_GC_STRESS").is_some_and(|s| s == "1") {
            let mut realm = crate::realms::enter_auto_realm(cx, doc.window());
            if let Err(error) = collect_joint(&mut realm.current_realm(), doc.window()) {
                eprintln!("MWR_NATIVE_ERROR microtask GC: {error}");
                return;
            }
            eprintln!("MWR_NATIVE_MICROTASK_GC");
        }
        let global = doc.global();
        run_a_script::<DomTypeHolder, _, _>(cx, &global, |cx| {
            let mut realm = crate::realms::enter_auto_realm(cx, &*global);
            let dormant = state.engine.borrow_mut().take();
            let Some(dormant) = dormant else {
                eprintln!("MWR_NATIVE_ERROR microtask ran during active VM");
                return;
            };
            let mut context = realm.current_realm();
            let mut engine = dormant.resume(Box::new(NativeAdapter::new(
                &mut context,
                state.registry.clone(),
            )));
            let result = engine.run_one_microtask(Some(self.serial));
            for error in engine.take_microtask_errors() {
                eprintln!("MWR_NATIVE_MICROTASK_ERROR {error}");
            }
            *state.engine.borrow_mut() = Some(engine.suspend());
            match result {
                Ok(true) => eprintln!("MWR_NATIVE_MICROTASK"),
                Ok(false) => {}
                Err(error) => eprintln!("MWR_NATIVE_ERROR {error:?}"),
            }
            queue_collection(&doc);
            module_loader::queue_observation(&doc);
        });
    }
}

pub(crate) fn finish_microtask_checkpoint() {
    let states: Vec<_> = SESSIONS.with(|s| s.borrow().values().cloned().collect());
    for state in states {
        if let Some(engine) = state.engine.borrow_mut().as_mut() {
            for message in engine.take_unhandled_rejections() {
                eprintln!("MWR_NATIVE_UNHANDLED_REJECTION {message}");
            }
        }
    }
}

pub(crate) fn execute_modules(
    cx: &mut JSContext,
    doc: &Document,
    text: &str,
    src: Option<String>,
    options: crate::modules::script_module::ScriptFetchOptions,
) {
    let global = doc.global();
    run_a_script::<DomTypeHolder, _, _>(cx, &global, |cx| match create_session(cx, doc, "") {
        Ok(state) => {
            if let Err(error) = module_loader::begin(cx, doc, &state, text, src, options) {
                module_loader::fail(cx, doc, &state, error);
            }
        }
        Err(error) => eprintln!("MWR_NATIVE_MODULE_ERROR {error}"),
    });
}
