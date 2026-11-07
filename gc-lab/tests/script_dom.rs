use gc_lab::{script::Engine, Model};
const SOURCE: &str = include_str!("../examples/button.js");
fn configurations() -> [(Model, Model); 4] {
    [
        (Model::Stable, Model::Stable),
        (Model::Stable, Model::Indirect),
        (Model::Indirect, Model::Stable),
        (Model::Indirect, Model::Indirect),
    ]
}
#[test]
fn requested_javascript_changes_text_and_releases_the_cross_heap_cycle() {
    for (dom, js) in configurations() {
        for stress in [false, true] {
            let mut e = Engine::new(dom, js).unwrap();
            e.set_stress(stress);
            e.execute(SOURCE).unwrap();
            assert_eq!(e.text_content("button").unwrap(), "Funciona");
            e.collect();
            assert_eq!(e.live_roots(), 1);
            let retained_graph = e.live_objects();
            let node = e.retain_node("button").unwrap();
            e.release_globals();
            assert_eq!(e.retained_text(node).unwrap(), "Funciona");
            assert_eq!(e.live_objects(), retained_graph);
            e.release_node(node).unwrap();
            assert_eq!(
                (
                    e.live_objects(),
                    e.metadata_objects(),
                    e.live_roots(),
                    e.charged_bytes()
                ),
                (0, 0, 0, 0)
            );
        }
    }
}
#[test]
fn closures_capture_local_objects_and_return_through_multiple_allocating_calls() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute("const make = (label) => { const n = document.createElement('button'); const handler = () => { n.textContent = label; }; n.addEventListener('click', handler); return n; }; const first = (a,b) => { return a; }; const button = first(make('retained'), make('discarded')); button.click();").unwrap();
    assert_eq!(e.text_content("button").unwrap(), "retained");
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
    assert_eq!(e.metadata_objects(), 0);
}
#[test]
fn listener_snapshot_respects_removal_and_does_not_run_new_listeners_yet() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute("const button = document.createElement('button'); const second = () => { button.textContent = 'wrong'; }; const late = () => { button.textContent = 'late'; }; const first = () => { button.removeEventListener('click', second); button.addEventListener('click', late); button.textContent = 'first'; }; button.addEventListener('click', first); button.addEventListener('click', second); button.click();").unwrap();
    assert_eq!(e.text_content("button").unwrap(), "first");
}
#[test]
fn handler_can_remove_itself_and_reenter_click_without_leaking_frames() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute("const button = document.createElement('button'); const handler = () => { button.removeEventListener('click', handler); button.click(); button.textContent = 'once'; }; button.addEventListener('click', handler); button.addEventListener('click', handler); button.click();").unwrap();
    assert_eq!(e.text_content("button").unwrap(), "once");
    assert_eq!(e.live_roots(), 1);
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn host_and_callback_errors_unwind_all_interpreter_roots() {
    for source in [
        "const button = document.createElement('button'); button.addEventListener('click', () => { missing(); }); button.click();",
        "const button = document.createElement('button'); const f = () => { button.click(); }; button.addEventListener('click', f); button.click();",
        "document.createElement('');", "const n = document.createElement('a'); n.textContent = () => {};",
        "const n = document.createElement('a'); n.addEventListener('keydown', () => {});",
    ] {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        assert!(e.execute(source).is_err()); e.collect();
        assert_eq!((e.live_objects(),e.metadata_objects(),e.live_roots()), (0,0,0), "{source}");
        e.execute(SOURCE).unwrap(); assert_eq!(e.text_content("button").unwrap(), "Funciona");
    }
}
#[test]
fn fault_injected_quotas_and_step_limits_release_partial_dom_and_callbacks() {
    for budget in [0, 100, 200, 400, 600, 800] {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.set_budget(Some(budget));
        let result = e.execute(SOURCE);
        if result.is_err() {
            assert_eq!(
                (e.live_objects(), e.metadata_objects(), e.live_roots()),
                (0, 0, 0)
            );
        }
        e.release_globals();
        assert_eq!(e.charged_bytes(), 0);
        e.set_budget(None);
        e.execute(SOURCE).unwrap();
    }
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.set_step_limit(3);
    assert!(e.execute(SOURCE).is_err());
    assert_eq!(e.live_roots(), 0);
    assert_eq!(e.live_objects(), 0);
}
