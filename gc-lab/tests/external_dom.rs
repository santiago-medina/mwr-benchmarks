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
}
struct Host(Rc<RefCell<State>>);
impl DomAdapter for Host {
    fn create_element(&mut self, _: &str) -> Result<u64, String> {
        let mut s = self.0.borrow_mut();
        s.next += 1;
        let id = s.next + 10;
        s.leases.insert(id);
        s.text.insert(id, String::new());
        Ok(id)
    }
    fn query_selector(&mut self, selector: &str) -> Result<u64, String> {
        if selector != "#button" {
            return Err("not found".into());
        }
        let mut s = self.0.borrow_mut();
        s.leases.insert(1);
        s.text.entry(1).or_insert("Pulsar".into());
        Ok(1)
    }
    fn tag_name(&mut self, _: u64) -> Result<String, String> {
        if self.0.borrow().fail_tag {
            Err("tag failed".into())
        } else {
            Ok("BUTTON".into())
        }
    }
    fn text_content(&mut self, node: u64) -> Result<String, String> {
        self.0
            .borrow()
            .text
            .get(&node)
            .cloned()
            .ok_or("missing".into())
    }
    fn set_text_content(&mut self, node: u64, text: &str) -> Result<(), String> {
        let mut s = self.0.borrow_mut();
        if s.fail_set {
            return Err("set failed".into());
        }
        s.text.insert(node, text.into());
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
fn engine() -> (Engine, Rc<RefCell<State>>) {
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
