use gc_lab::{
    dual::Runtime,
    script::{wire, DomAdapter, Engine},
    Model,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
#[derive(Default)]
struct State {
    leases: HashSet<u64>,
    hooks: HashSet<u64>,
    text: HashMap<u64, String>,
    next: u64,
    clicks: usize,
    releases: Vec<u64>,
    fail_release: bool,
    fail_tag: bool,
    fail_listener: bool,
    fail_set: bool,
    traced: bool,
    children: HashMap<u64, Vec<u64>>,
}
struct Host(Rc<RefCell<State>>);
impl DomAdapter for Host {
    fn traces_native_edges(&self) -> bool {
        self.0.borrow().traced
    }
    fn create_element(&mut self, _: &str) -> Result<u64, String> {
        let mut s = self.0.borrow_mut();
        s.next += 1;
        let id = s.next + 10;
        s.leases.insert(id);
        s.text.insert(id, String::new());
        Ok(id)
    }
    fn query_selector(&mut self, selector: &str) -> Result<u64, String> {
        let id = match selector {
            "#button" => 1,
            "#complete" => 2,
            "#remove" => 3,
            "#list" => 4,
            _ => return Err("not found".into()),
        };
        let mut s = self.0.borrow_mut();
        s.leases.insert(id);
        s.text.entry(id).or_insert(if id == 1 {
            "Pulsar".into()
        } else {
            String::new()
        });
        Ok(id)
    }

    fn tag_name(&mut self, _: u64) -> Result<String, String> {
        if self.0.borrow().fail_tag {
            Err("tag failed".into())
        } else {
            Ok("BUTTON".into())
        }
    }
    fn text_content(&mut self, node: u64) -> Result<String, String> {
        fn read(s: &State, node: u64) -> String {
            let mut text = s.text.get(&node).cloned().unwrap_or_default();
            if let Some(children) = s.children.get(&node) {
                for child in children {
                    text.push_str(&read(s, *child));
                }
            }
            text
        }
        Ok(read(&self.0.borrow(), node))
    }

    fn set_text_content(&mut self, node: u64, text: &str) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        if s.fail_set {
            return Err("set failed".into());
        }
        s.text.insert(node, text.into());
        Ok(())
    }
    fn append_child(&mut self, parent: u64, child: u64) -> Result<(), String> {
        self.0
            .borrow_mut()
            .children
            .entry(parent)
            .or_default()
            .push(child);
        Ok(())
    }
    fn remove_child(&mut self, parent: u64, child: u64) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        let children = s.children.entry(parent).or_default();
        let index = children
            .iter()
            .position(|id| *id == child)
            .ok_or("not a child")?;
        children.remove(index);
        Ok(())
    }
    fn set_click_listener(&mut self, node: u64, enabled: bool) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        if s.fail_listener {
            return Err("listen failed".into());
        }
        if enabled {
            s.hooks.insert(node);
        } else {
            s.hooks.remove(&node);
        }
        Ok(())
    }
    fn click(&mut self, node: u64) -> Result<Vec<u64>, String> {
        let mut s = self.0.borrow_mut();
        s.clicks += 1;
        Ok(if s.hooks.contains(&node) {
            vec![node]
        } else {
            vec![]
        })
    }
    fn release(&mut self, nodes: &[u64]) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        if s.fail_release {
            return Err("release failed".into());
        }
        for node in nodes {
            s.leases.remove(node);
            s.hooks.remove(node);
            s.releases.push(*node);
        }
        Ok(())
    }
}
const SOURCE: &str = "const button = document.querySelector('#button'); button.addEventListener('click', () => { button.textContent = 'Funciona'; });";
fn engine() -> (Engine<'static>, Rc<RefCell<State>>) {
    let state = Rc::new(RefCell::new(State::default()));
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.set_dom_adapter(Box::new(Host(state.clone()))).unwrap();
    (e, state)
}
#[test]
fn external_event_runs_our_closure_and_releases_foreign_lease() {
    let (mut e, state) = engine();
    e.execute(SOURCE).unwrap();
    assert_eq!(state.borrow().text[&1], "Pulsar");
    e.dispatch_remote_click(1).unwrap();
    assert_eq!(e.text_content("button").unwrap(), "Funciona");
    assert_eq!(state.borrow().clicks, 0); // External delivery must not synthesize a new click.
    e.release_globals();
    assert_eq!(e.remote_node_count(), 0);
    assert!(state.borrow().leases.is_empty());
    assert!(state.borrow().hooks.is_empty());
    assert!(e.dispatch_remote_click(1).is_err());
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn programmatic_click_and_repeated_lookup_preserve_node_identity() {
    let (mut e, state) = engine();
    e.execute("const button = document.querySelector('#button'); const same = document.querySelector('#button'); same.addEventListener('click', () => { button.textContent = 'same'; }); button.click();").unwrap();
    assert_eq!(e.text_content("same").unwrap(), "same");
    assert_eq!(state.borrow().clicks, 1);
    assert_eq!(e.remote_node_count(), 1);
    e.release_globals();
    assert_eq!(state.borrow().releases, vec![1]);
}
#[test]
fn allocation_and_registration_failures_release_new_foreign_nodes() {
    let (mut e, state) = engine();
    e.set_budget(Some(Runtime::allocation_charge(64).unwrap()));
    assert!(e.execute(SOURCE).is_err());
    assert!(state.borrow().leases.is_empty());
    assert_eq!(e.remote_node_count(), 0);
    e.set_budget(None);
    state.borrow_mut().fail_listener = true;
    assert!(e.execute(SOURCE).is_err());
    assert!(state.borrow().leases.is_empty());
    assert_eq!(e.live_roots(), 0);
    state.borrow_mut().fail_listener = false;
    state.borrow_mut().fail_tag = true;
    assert!(e.execute(SOURCE).is_err());
    assert!(state.borrow().leases.is_empty());
}
#[test]
fn failed_release_is_retained_and_retried_instead_of_forgotten() {
    let (mut e, state) = engine();
    e.execute(SOURCE).unwrap();
    state.borrow_mut().fail_release = true;
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
    assert_eq!(e.remote_node_count(), 1);
    assert!(e.host_error().is_some());
    assert!(!state.borrow().leases.is_empty());
    state.borrow_mut().fail_release = false;
    e.collect();
    assert_eq!(e.remote_node_count(), 0);
    assert!(e.host_error().is_none());
    assert!(state.borrow().leases.is_empty());
}
#[test]
fn failed_remote_mutation_is_reported_and_programmatic_path_unwinds() {
    let (mut e, state) = engine();
    state.borrow_mut().fail_set = true;
    assert!(e.execute(&format!("{SOURCE} button.click();")).is_err());
    assert_eq!(state.borrow().text[&1], "Pulsar");
    assert!(state.borrow().leases.is_empty());
    assert_eq!(e.live_roots(), 0);
}
#[test]
fn data_wire_roundtrips_without_turning_text_into_source() {
    for text in [
        "",
        "quotes '\" \\ tabs\t newline\n",
        "Sí — 日本語",
        "</script><script>throw 'x'</script>",
    ] {
        let encoded = wire::encode(text);
        assert!(!encoded.contains(['\t', '\n', '<']));
        assert_eq!(wire::decode(&encoded).unwrap(), text);
    }
    for invalid in ["a", "zz", "ff", "0g"] {
        assert!(wire::decode(invalid).is_err());
    }
}

impl DomAdapter for &mut Host {
    fn traces_native_edges(&self) -> bool {
        (**self).traces_native_edges()
    }
    fn create_element(&mut self, tag: &str) -> Result<u64, String> {
        (**self).create_element(tag)
    }
    fn query_selector(&mut self, selector: &str) -> Result<u64, String> {
        (**self).query_selector(selector)
    }
    fn tag_name(&mut self, node: u64) -> Result<String, String> {
        (**self).tag_name(node)
    }
    fn text_content(&mut self, node: u64) -> Result<String, String> {
        (**self).text_content(node)
    }
    fn set_text_content(&mut self, node: u64, text: &str) -> Result<(), String> {
        (**self).set_text_content(node, text)
    }
    fn set_click_listener(&mut self, node: u64, enabled: bool) -> Result<(), String> {
        (**self).set_click_listener(node, enabled)
    }
    fn click(&mut self, node: u64) -> Result<Vec<u64>, String> {
        (**self).click(node)
    }
    fn release(&mut self, nodes: &[u64]) -> Result<(), String> {
        (**self).release(nodes)
    }
}

#[test]
fn borrowed_host_context_can_end_between_script_and_native_event() {
    let state = Rc::new(RefCell::new(State::default()));
    let mut host = Host(state.clone());
    let suspended = {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.set_dom_adapter(Box::new(&mut host)).unwrap();
        e.execute(SOURCE).unwrap();
        e.suspend()
    };
    // The host borrow has ended, but the VM closure and lease are still alive.
    assert_eq!(host.text_content(1).unwrap(), "Pulsar");
    assert!(state.borrow().hooks.contains(&1));
    let suspended = {
        let mut e = suspended.resume(Box::new(&mut host));
        e.dispatch_remote_click(1).unwrap();
        assert_eq!(e.text_content("button").unwrap(), "Funciona");
        e.suspend()
    };
    assert_eq!(host.text_content(1).unwrap(), "Funciona");
    let mut e = suspended.resume(Box::new(&mut host));
    e.release_globals();
    assert_eq!(e.remote_node_count(), 0);
    assert_eq!(e.live_objects(), 0);
    assert!(state.borrow().leases.is_empty());
    assert!(state.borrow().hooks.is_empty());
}

#[test]
fn native_marking_keeps_a_scoped_listener_alive_then_reclaims_the_cycle() {
    let (mut e, state) = engine();
    state.borrow_mut().traced = true;
    e.execute("const setup = () => { const button = document.querySelector('#button'); button.addEventListener('click', () => { button.textContent = 'Funciona'; }); }; ").unwrap();
    let baseline = e.live_objects();
    e.call_global("setup").unwrap();
    e.collect();
    assert!(e.live_objects() > baseline);
    let suspended = e.suspend();
    assert!(suspended.trace_native_edges(None).is_empty());
    assert_eq!(suspended.trace_native_edges(Some(1)), vec![1]);
    let mut e = suspended.resume(Box::new(Host(state.clone())));
    e.collect_native(&[1]).unwrap();
    e.dispatch_remote_click(1).unwrap();
    assert_eq!(state.borrow().text[&1], "Funciona");
    // The native collector reports no incoming edge after the DOM node dies.
    e.collect_native(&[]).unwrap();
    assert_eq!(e.live_objects(), baseline);
    assert_eq!(e.metadata_objects(), baseline);
    assert_eq!(e.live_roots(), 1); // Realm remains alive throughout.
    assert_eq!(e.remote_node_count(), 0);
    assert!(state.borrow().hooks.is_empty());
    assert!(e.dispatch_remote_click(1).is_err());
    // Repeating setup must not depend on a discarded realm or stale IDs.
    for _ in 0..100 {
        e.call_global("setup").unwrap();
        e.collect_native(&[1]).unwrap();
        e.dispatch_remote_click(1).unwrap();
        e.collect_native(&[]).unwrap();
        assert_eq!(e.live_objects(), baseline);
    }
}

#[test]
fn vm_roots_and_foreign_callbacks_trace_transitive_native_edges() {
    let (mut e, state) = engine();
    state.borrow_mut().traced = true;
    e.execute("const keep = document.createElement('button'); const setup = () => { const a = document.querySelector('#button'); const b = document.createElement('button'); a.addEventListener('click', () => { b.textContent = 'kept'; }); }; ").unwrap();
    let baseline = e.live_objects();
    e.call_global("setup").unwrap();
    let suspended = e.suspend();
    let roots = suspended.trace_native_edges(None);
    assert_eq!(roots.len(), 1); // Detached node retained by our JS global.
    let native = suspended.trace_native_edges(Some(1));
    assert!(native.contains(&1));
    assert!(native.len() >= 2); // Native listener -> VM closure -> second node.
    let mut e = suspended.resume(Box::new(Host(state.clone())));
    e.collect_native(&native).unwrap();
    e.dispatch_remote_click(1).unwrap();
    assert!(state.borrow().text.values().any(|v| v == "kept"));
    e.collect_native(&[]).unwrap();
    assert_eq!(e.live_objects(), baseline);
    assert_eq!(e.remote_node_count(), 1);
    assert!(e.text_content("keep").is_ok());
}

#[test]
fn throwing_setup_unwinds_frames_without_losing_a_native_listener() {
    let (mut e, state) = engine();
    state.borrow_mut().traced = true;
    e.execute("const setup = () => { const button = document.querySelector('#button'); button.addEventListener('click', () => { button.textContent = 'after error'; }); button.unsupported(); }; ").unwrap();
    let baseline = e.live_objects();
    assert!(e.call_global("setup").is_err());
    assert_eq!(e.live_roots(), 1);
    e.collect_native(&[1]).unwrap();
    e.dispatch_remote_click(1).unwrap();
    assert_eq!(state.borrow().text[&1], "after error");
    e.collect_native(&[]).unwrap();
    assert_eq!(e.live_objects(), baseline);
    assert_eq!(e.remote_node_count(), 0);
}

#[test]
fn object_state_counter_uses_native_receiver_and_reclaims_graph() {
    for model in [Model::Stable, Model::Indirect] {
        let state = Rc::new(RefCell::new(State::default()));
        let mut e = Engine::new(model, model).unwrap();
        e.set_dom_adapter(Box::new(Host(state.clone()))).unwrap();
        e.execute(include_str!("../examples/object-counter.js"))
            .unwrap();
        assert_eq!(e.text_content("button").unwrap(), "Cuenta: 0");
        e.dispatch_remote_click(1).unwrap();
        assert_eq!(e.text_content("button").unwrap(), "Cuenta: 1");
        e.collect();
        e.dispatch_remote_click(1).unwrap();
        assert_eq!(e.text_content("button").unwrap(), "Cuenta: 2");
        e.release_globals();
        assert_eq!(
            (e.live_objects(), e.metadata_objects(), e.live_roots()),
            (0, 0, 0)
        );
        assert!(state.borrow().leases.is_empty());
    }
}
#[test]
fn host_mutation_error_is_catchable_and_does_not_corrupt_dom() {
    let state = Rc::new(RefCell::new(State::default()));
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.set_dom_adapter(Box::new(Host(state.clone()))).unwrap();
    e.execute("const button=document.querySelector('#button');const state={caught:''};button.addEventListener('click',function(){try{this.textContent='new';}catch(error){state.caught=error.message;}});function render(){button.textContent=state.caught;}").unwrap();
    state.borrow_mut().fail_set = true;
    e.dispatch_remote_click(1).unwrap();
    assert_eq!(e.text_content("button").unwrap(), "Pulsar");
    state.borrow_mut().fail_set = false;
    e.call_global("render").unwrap();
    assert_eq!(e.text_content("button").unwrap(), "set failed");
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn task_list_adds_completes_removes_nodes_and_releases_array_captures() {
    for model in [Model::Stable, Model::Indirect] {
        let state = Rc::new(RefCell::new(State::default()));
        let mut e = Engine::new(model, model).unwrap();
        e.set_dom_adapter(Box::new(Host(state.clone()))).unwrap();
        e.execute(include_str!("../examples/task-list.js")).unwrap();
        e.dispatch_remote_click(1).unwrap();
        assert_eq!(e.text_content("list").unwrap(), "Tarea 1");
        e.dispatch_remote_click(2).unwrap();
        assert_eq!(e.text_content("list").unwrap(), "[hecha] Tarea 1");
        e.dispatch_remote_click(3).unwrap();
        assert_eq!(e.text_content("list").unwrap(), "");
        e.collect();
        assert!(!state.borrow().leases.contains(&11));
        e.dispatch_remote_click(1).unwrap();
        assert_eq!(e.text_content("list").unwrap(), "Tarea 2");
        e.release_globals();
        assert_eq!(
            (e.live_objects(), e.metadata_objects(), e.live_roots()),
            (0, 0, 0)
        );
        assert!(state.borrow().leases.is_empty());
        assert!(state.borrow().hooks.is_empty());
    }
}
#[test]
fn each_external_event_gets_a_fresh_execution_budget() {
    let (mut e, _) = engine();
    e.execute(include_str!("../examples/object-counter.js"))
        .unwrap();
    e.set_step_limit(200);
    for _ in 0..50 {
        e.dispatch_remote_click(1).unwrap();
    }
    assert_eq!(e.text_content("button").unwrap(), "Cuenta: 50");
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn queued_job_alone_keeps_a_foreign_node_then_releases_it() {
    let (mut e, state) = engine();
    e.enable_test_harness();
    e.execute("function make(){const node=document.createElement('span');node.textContent='kept';Promise.resolve(node).then(function(n){assert.sameValue(n.textContent,'kept');n.textContent='done';});}make();").unwrap();
    e.collect();
    assert!(state.borrow().leases.contains(&11));
    e.microtask_checkpoint(100).unwrap();
    assert_eq!(e.assertion_count(), 1);
    assert_eq!(state.borrow().text[&11], "done");
    assert!(!state.borrow().leases.contains(&11));
    e.release_globals();
    assert_eq!((e.live_objects(), e.live_roots()), (0, 0));
}
