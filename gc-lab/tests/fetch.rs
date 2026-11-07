use gc_lab::{
    script::{DomAdapter, Engine, FetchResponse, ScriptError},
    Model,
};
use std::{cell::RefCell, rc::Rc};
#[derive(Default)]
struct State {
    requests: Vec<(u64, String)>,
    bodies: Vec<(u64, u64)>,
    canceled: Vec<u64>,
    released: Vec<u64>,
}
struct Host(Rc<RefCell<State>>);
impl DomAdapter for Host {
    fn supports_fetch(&self) -> bool {
        true
    }
    fn start_fetch(&mut self, s: u64, url: &str) -> Result<(), String> {
        self.0.borrow_mut().requests.push((s, url.into()));
        Ok(())
    }
    fn read_response_text(&mut self, s: u64, h: u64) -> Result<(), String> {
        self.0.borrow_mut().bodies.push((s, h));
        Ok(())
    }
    fn cancel_fetch(&mut self, s: u64) {
        self.0.borrow_mut().canceled.push(s);
    }
    fn release_responses(&mut self, h: &[u64]) -> Result<(), String> {
        self.0.borrow_mut().released.extend(h);
        Ok(())
    }
    fn create_element(&mut self, _: &str) -> Result<u64, String> {
        unreachable!()
    }
    fn query_selector(&mut self, _: &str) -> Result<u64, String> {
        unreachable!()
    }
    fn tag_name(&mut self, _: u64) -> Result<String, String> {
        unreachable!()
    }
    fn text_content(&mut self, _: u64) -> Result<String, String> {
        unreachable!()
    }
    fn set_text_content(&mut self, _: u64, _: &str) -> Result<(), String> {
        unreachable!()
    }
    fn set_click_listener(&mut self, _: u64, _: bool) -> Result<(), String> {
        unreachable!()
    }
    fn click(&mut self, _: u64) -> Result<Vec<u64>, String> {
        unreachable!()
    }
    fn release(&mut self, _: &[u64]) -> Result<(), String> {
        Ok(())
    }
}
fn engine(model: Model) -> (Engine<'static>, Rc<RefCell<State>>) {
    let state = Rc::new(RefCell::new(State::default()));
    let mut e = Engine::new(model, model).unwrap();
    e.set_dom_adapter(Box::new(Host(state.clone()))).unwrap();
    e.enable_test_harness();
    (e, state)
}
fn drain(e: &mut Engine<'_>) {
    e.collect();
    e.microtask_checkpoint(100).unwrap();
    assert!(
        e.unhandled_test_failure().is_none(),
        "{:?}",
        e.unhandled_test_failure()
    );
    assert!(e.take_microtask_errors().is_empty());
}
fn finish(e: &mut Engine<'_>) {
    e.release_globals();
    assert_eq!(
        (
            e.live_objects(),
            e.metadata_objects(),
            e.live_roots(),
            e.charged_bytes()
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(e.pending_fetches(), 0);
}
fn headers(handle: u64, status: u16) -> FetchResponse {
    FetchResponse {
        handle,
        status,
        url: "https://example.test/data".into(),
    }
}
#[test]
fn headers_and_body_complete_separately_under_gc() {
    for model in [Model::Stable, Model::Indirect] {
        let (mut e, s) = engine(model);
        e.execute("let stage='start';async function run(){const r=await fetch('/data');assert.sameValue(r.status,200);assert.sameValue(r.ok,true);assert.sameValue(r.url,'https://example.test/data');assert.sameValue(r.bodyUsed,false);stage='headers';const p=r.text();assert.sameValue(r.bodyUsed,true);const text=await p;assert.sameValue(text,'hello');stage='body';}run();").unwrap();
        let request = s.borrow().requests[0].0;
        assert_eq!(e.binding_string("stage").unwrap(), "start");
        assert_eq!(e.pending_fetches(), 1);
        e.collect();
        assert!(e.complete_fetch(request, Ok(headers(77, 200))).unwrap());
        assert_eq!(e.binding_string("stage").unwrap(), "start");
        drain(&mut e);
        assert_eq!(e.binding_string("stage").unwrap(), "headers");
        let body = s.borrow().bodies[0].0;
        e.collect();
        assert!(s.borrow().released.is_empty());
        assert!(e.complete_response_text(body, Ok("hello".into())).unwrap());
        drain(&mut e);
        assert_eq!(e.binding_string("stage").unwrap(), "body");
        assert_eq!(e.pending_fetches(), 0);
        finish(&mut e);
        assert_eq!(s.borrow().released, vec![77]);
    }
}
#[test]
fn http_error_fulfills_but_network_and_body_errors_reject() {
    for model in [Model::Stable, Model::Indirect] {
        let (mut e, s) = engine(model);
        e.execute("async function run(){const r=await fetch('/missing');assert.sameValue(r.status,404);assert.sameValue(r.ok,false);try{await r.text();assert(false);}catch(e){assert.sameValue(e.constructor,TypeError);}try{await fetch('/offline');assert(false);}catch(e){assert.sameValue(e.constructor,TypeError);}}run();").unwrap();
        let serial = s.borrow().requests[0].0;
        e.complete_fetch(serial, Ok(headers(8, 404))).unwrap();
        drain(&mut e);
        let serial = s.borrow().bodies[0].0;
        e.complete_response_text(serial, Err("body failure".into()))
            .unwrap();
        drain(&mut e);
        let serial = s.borrow().requests[1].0;
        e.complete_fetch(serial, Err("network failure".into()))
            .unwrap();
        drain(&mut e);
        assert_eq!(e.assertion_count(), 4);
        finish(&mut e);
    }
}
#[test]
fn body_consumption_is_once_only_and_checks_receiver() {
    for model in [Model::Stable, Model::Indirect] {
        let (mut e, s) = engine(model);
        e.execute("async function run(){const r=await fetch('/data');const p=r.text();r.text().catch(function(e){assert.sameValue(e.constructor,TypeError);});r.text.call({}).catch(function(e){assert.sameValue(e.constructor,TypeError);});const get=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(r),'status').get;assert.throws(TypeError,function(){get.call({});});await p;}run();").unwrap();
        let serial = s.borrow().requests[0].0;
        e.complete_fetch(serial, Ok(headers(1, 200))).unwrap();
        drain(&mut e);
        assert_eq!(s.borrow().bodies.len(), 1);
        let serial = s.borrow().bodies[0].0;
        e.complete_response_text(serial, Ok("".into())).unwrap();
        drain(&mut e);
        assert_eq!(e.assertion_count(), 3);
        finish(&mut e);
    }
}
#[test]
fn realm_release_cancels_pending_operations_and_ignores_late_tokens() {
    for model in [Model::Stable, Model::Indirect] {
        let (mut e, s) = engine(model);
        e.execute("async function f(){await fetch('/slow');assert(false);}f();")
            .unwrap();
        let serial = s.borrow().requests[0].0;
        finish(&mut e);
        assert_eq!(s.borrow().canceled, vec![serial]);
        assert!(!e.complete_fetch(serial, Err("late".into())).unwrap());
        e.execute("async function f(){await fetch('/new');}f();")
            .unwrap();
        let new = s.borrow().requests[1].0;
        assert!(new > serial);
        assert!(!e.complete_fetch(serial, Err("late again".into())).unwrap());
        assert_eq!(e.pending_fetches(), 1);
        finish(&mut e);
    }
}
#[test]
fn body_request_keeps_response_alive_and_navigation_releases_it() {
    for model in [Model::Stable, Model::Indirect] {
        let (mut e, s) = engine(model);
        e.execute("fetch('/data').then(function(r){r.text();});")
            .unwrap();
        let serial = s.borrow().requests[0].0;
        e.complete_fetch(serial, Ok(headers(19, 200))).unwrap();
        drain(&mut e);
        assert_eq!(e.pending_fetches(), 1);
        assert!(s.borrow().released.is_empty());
        let body = s.borrow().bodies[0].0;
        finish(&mut e);
        assert!(s.borrow().canceled.contains(&body));
        assert_eq!(s.borrow().released, vec![19]);
        assert!(!e.complete_response_text(body, Ok("late".into())).unwrap());
    }
}
#[test]
fn unsupported_transport_and_options_are_explicit() {
    let mut e = Engine::new(Model::Stable, Model::Stable).unwrap();
    assert!(matches!(
        e.execute("fetch('/data');"),
        Err(ScriptError::Unsupported(_))
    ));
    finish(&mut e);
    let (mut e, _) = engine(Model::Indirect);
    assert!(matches!(
        e.execute("fetch('/data',{});"),
        Err(ScriptError::Unsupported(_))
    ));
    finish(&mut e);
}
