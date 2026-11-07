use gc_lab::{
    script::{Engine, ScriptError},
    Model,
};
fn engine() -> Engine {
    Engine::new(Model::Indirect, Model::Indirect).unwrap()
}
#[test]
fn closure_returns_a_captured_value_after_its_call_frame_has_gone() {
    let mut e = engine();
    e.execute("const make = (x) => { return () => { return x; }; }; const f = make('captured'); const result = f();").unwrap();
    assert_eq!(e.binding_string("result").unwrap(), "captured");
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
    assert_eq!(e.metadata_objects(), 0);
    assert_eq!(e.live_roots(), 0);
}
#[test]
fn lexical_scope_shadowing_and_left_to_right_arguments() {
    let mut e = engine();
    e.execute("const x = 'outer'; const f = (x) => { return x; }; const result = f('inner');")
        .unwrap();
    assert_eq!(e.binding_string("result").unwrap(), "inner");
    assert_eq!(e.binding_string("x").unwrap(), "outer");
}
#[test]
fn runtime_errors_clean_the_realm_and_preserve_no_execution_roots() {
    for source in [
        "const x = x;",
        "const x = 'a'; const x = 'b';",
        "missing();",
        "return 'a';",
        "const x = 'a'; x();",
        "const f = (x) => { const x = 'a'; }; f('b');",
    ] {
        let mut e = engine();
        assert!(e.execute(source).is_err(), "{source}");
        e.collect();
        assert_eq!(e.live_objects(), 0, "{source}");
        assert_eq!(e.metadata_objects(), 0);
        assert_eq!(e.live_roots(), 0);
    }
}
#[test]
fn recursive_closure_is_bounded_and_memory_errors_are_recoverable() {
    let mut e = engine();
    assert!(e.execute("const f = () => { return f(); }; f();").is_err());
    assert_eq!(e.live_objects(), 0);
    assert_eq!(e.live_roots(), 0);
    e.set_budget(Some(0));
    assert!(matches!(
        e.execute("const x = 'a';"),
        Err(ScriptError::Memory(_))
    ));
    e.set_budget(None);
    e.execute("const x = 'works';").unwrap();
    assert_eq!(e.binding_string("x").unwrap(), "works");
}
#[test]
fn object_results_remain_alive_during_later_argument_allocations() {
    let mut e = engine();
    e.execute("const first = (a,b) => { return a; }; const make = () => { return () => { return 'kept'; }; }; const f = first(make(), make()); const result = f();").unwrap();
    assert_eq!(e.binding_string("result").unwrap(), "kept");
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
