//! Opt-in experiment. Our AST and closures run on Servo's script thread.
//! SpiderMonkey still owns Servo's DOM reflectors; this is not a replacement VM.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gc_lab::Model;
use gc_lab::script::{DomAdapter, Engine, SuspendedEngine};
use js::context::JSContext;
use servo_base::id::PipelineId;

use crate::dom::bindings::codegen::Bindings::DocumentBinding::DocumentMethods;
use crate::dom::bindings::codegen::Bindings::ElementBinding::ElementMethods;
use crate::dom::bindings::codegen::Bindings::HTMLElementBinding::HTMLElementMethods;
use crate::dom::bindings::codegen::Bindings::NodeBinding::NodeMethods;
use crate::dom::bindings::codegen::UnionTypes::StringOrElementCreationOptions;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::document::Document;
use crate::dom::element::Element;
use crate::dom::eventtarget::EventTarget;
use crate::dom::html::htmlelement::HTMLElement;
use crate::dom::node::Node;
use crate::dom::window::Window;

type Result<T> = std::result::Result<T, String>;
thread_local! {
    static SESSIONS: RefCell<HashMap<PipelineId, Rc<Session>>> = RefCell::new(HashMap::new());
}
struct Session {
    engine: RefCell<Option<SuspendedEngine>>,
    registry: Rc<RefCell<Registry>>,
}
struct Registry {
    document: Trusted<Document>,
    nodes: HashMap<u64, Trusted<Node>>,
    next: u64,
}
impl Registry {
    fn intern(&mut self, node: &Node) -> Result<u64> {
        for (id, trusted) in &self.nodes {
            if &*trusted.root() == node {
                return Ok(*id);
            }
        }
        let id = self.next;
        self.next = id.checked_add(1).ok_or("native ID space exhausted")?;
        self.nodes.insert(id, Trusted::new(node));
        Ok(id)
    }
    fn node(&self, id: u64) -> Result<DomRoot<Node>> {
        self.nodes
            .get(&id)
            .map(Trusted::root)
            .ok_or("released native node".into())
    }
}
// Borrow the ONE current context, only for this invocation. Never manufacture a
// second JSContext or hold the context across event-loop turns.
struct NativeAdapter<'cx> {
    cx: &'cx mut JSContext,
    registry: Rc<RefCell<Registry>>,
}
impl DomAdapter for NativeAdapter<'_> {
    fn create_element(&mut self, tag: &str) -> Result<u64> {
        let doc = self.registry.borrow().document.root();
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
        let doc = self.registry.borrow().document.root();
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
                node.root()
                    .upcast::<EventTarget>()
                    .set_mwr_listener(*id, false);
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
    if let Err(error) = execute_inner(cx, doc, source) {
        eprintln!("MWR_NATIVE_ERROR {error}");
        clear(doc.window());
    }
}
fn execute_inner(cx: &mut JSContext, doc: &Document, source: &str) -> Result<()> {
    let window = doc.window();
    if session(window).is_some() {
        return Err("only one experimental script per document is supported".into());
    }
    let state = Rc::new(Session {
        engine: RefCell::new(None),
        registry: Rc::new(RefCell::new(Registry {
            document: Trusted::new(doc),
            nodes: HashMap::new(),
            next: 1,
        })),
    });
    let mut engine = Engine::new(Model::Indirect, Model::Indirect).map_err(|e| format!("{e:?}"))?;
    engine
        .set_dom_adapter(Box::new(NativeAdapter {
            cx,
            registry: state.registry.clone(),
        }))
        .map_err(|e| format!("{e:?}"))?;
    engine.execute(source).map_err(|e| format!("{e:?}"))?;
    *state.engine.borrow_mut() = Some(engine.suspend());
    SESSIONS.with(|s| s.borrow_mut().insert(window.pipeline_id(), state));
    eprintln!("MWR_NATIVE_EXECUTED");
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
    let window = global.downcast::<Window>().ok_or("not a window")?;
    let state = session(window).ok_or("no native session")?;
    let dormant = state
        .engine
        .borrow_mut()
        .take()
        .ok_or("native callback reentry is not supported")?;
    let mut engine = dormant.resume(Box::new(NativeAdapter {
        cx,
        registry: state.registry.clone(),
    }));
    let result = engine
        .dispatch_remote_click(id)
        .map_err(|e| format!("{e:?}"));
    *state.engine.borrow_mut() = Some(engine.suspend());
    result?;
    eprintln!("MWR_NATIVE_CALLBACK");
    Ok(())
}
// This exercises Servo's real DOM event dispatcher in Rust, not a hardware input
// event. Ordinary human clicks use the same foreign-listener branch.
fn self_test(cx: &mut JSContext, doc: &Document) -> Result<()> {
    let button = doc
        .QuerySelector(cx, DOMString::from("#button"))
        .map_err(|e| format!("{e:?}"))?
        .ok_or("test button missing")?;
    let node = button.upcast::<Node>();
    let before = node.GetTextContent().unwrap_or_default().to_string();
    if before != "Pulsar" {
        return Err(format!("unexpected initial text: {before}"));
    }
    button
        .downcast::<HTMLElement>()
        .ok_or("test target not HTML")?
        .Click(cx);
    let after = node.GetTextContent().unwrap_or_default().to_string();
    if after != "Funciona" {
        return Err(format!("native callback failed: {after}"));
    }
    let state = session(doc.window()).ok_or("test session disappeared")?;
    let dormant = state.engine.borrow_mut().take().ok_or("test engine busy")?;
    let mut engine = dormant.resume(Box::new(NativeAdapter {
        cx,
        registry: state.registry.clone(),
    }));
    engine.release_globals();
    let objects = engine.live_objects();
    let roots = engine.live_roots();
    let wrappers = engine.remote_node_count();
    let metadata = engine.metadata_objects();
    if let Some(error) = engine.host_error() {
        return Err(error.into());
    }
    *state.engine.borrow_mut() = Some(engine.suspend());
    let leases = state.registry.borrow().nodes.len();
    if (objects, roots, wrappers, metadata, leases) != (0, 0, 0, 0, 0) {
        return Err("native teardown leaked VM state or DOM leases".into());
    }
    // After GC the same dispatcher must no longer invoke a released callback.
    button.downcast::<HTMLElement>().unwrap().Click(cx);
    drop(state);
    clear(doc.window());
    eprintln!(
        "MWR_NATIVE_PROOF {{\"before\":\"Pulsar\",\"after\":\"Funciona\",\"objects\":0,\"roots\":0,\"wrappers\":0,\"metadata\":0,\"leases\":0,\"click\":\"rust-dom-synthetic\"}}"
    );
    Ok(())
}
// Trusted DOM roots must go before Servo tears down its JS runtime. This hook is
// mandatory on navigation/shutdown; the registry is not a joint tracing GC.
pub(crate) fn clear(window: &Window) {
    if let Some(state) = SESSIONS.with(|s| s.borrow_mut().remove(&window.pipeline_id())) {
        for (id, node) in state.registry.borrow_mut().nodes.drain() {
            node.root()
                .upcast::<EventTarget>()
                .set_mwr_listener(id, false);
        }
    }
}

// Panic teardown must only drop owned roots: DOM methods may no longer be safe.
pub(crate) fn discard(window: &Window) {
    SESSIONS.with(|s| {
        s.borrow_mut().remove(&window.pipeline_id());
    });
}
