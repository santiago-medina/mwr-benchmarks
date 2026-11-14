use gc_lab::{
    script::{Engine, ScriptError},
    Model,
};
fn engine() -> Engine<'static> {
    Engine::new(Model::Indirect, Model::Indirect).unwrap()
}
#[test]
fn sloppy_eval_in_arrow_parameter_initializer_captures_local_var_environment() {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        let source = r#"
            const original = globalThis.arguments;
            const f = (p = eval("var arguments = 'param'")) => {
                assert.sameValue(arguments, 'param');
            };
            f();
            const g = (p = eval("var arguments = 'param'")) => {
                var arguments = 'local';
                assert.sameValue(arguments, 'local');
            };
            g();
            assert.sameValue(globalThis.arguments, original);
        "#;
        e.execute(source)
            .unwrap_or_else(|error| panic!("{error:?}\\n{source}"));
        assert_eq!(e.assertion_count(), 3);
        e.collect();
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
    }
}
#[test]
fn direct_eval_var_shadowing_preserves_original_assignment_reference() {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        let source = r#"
            function compound() {
                var x = 3;
                var inner = (function() {
                    x *= (eval("var x = 2;"), 4);
                    return x;
                })();
                assert.sameValue(inner, 2);
                assert.sameValue(x, 12);
            }
            function assignment() {
                var x = 3;
                var inner = (function() {
                    x = (eval("var x = 2;"), 4);
                    return x;
                })();
                assert.sameValue(inner, 2);
                assert.sameValue(x, 4);
            }
            compound(); assignment();
        "#;
        e.execute(source)
            .unwrap_or_else(|error| panic!("{error:?}\\n{source}"));
        assert_eq!(e.assertion_count(), 4);
        e.collect();
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
    }
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
#[test]
fn strings_compare_by_code_units_and_key_maps_by_code_units() {
    // §7.2.13 IsLessThan step 3: strings compare by code units, so U+FFFF (one unit 0xFFFF)
    // sorts after U+10000 (lead 0xD800). Literature check #335.
    let mut e = engine();
    e.execute(
        "const big = '\\u{10000}', top = '\\uFFFF', lone = '\\uD800';
         const m = new Map([[lone, 1], [lone + '\\uDC00', 2]]);
         const s = new Set([lone, '\\uD800', big, lone + '\\uDC00']);
         const result = [top > big, big < top, lone < big, big.length, m.get('\\uD800'),
           m.get(big), s.size, s.has('\\uD800\\uDC00'), [top, big, lone].sort().map(x => x.charCodeAt(0).toString(16)).join()].join();",
    )
    .unwrap();
    assert_eq!(
        e.binding_string("result").unwrap(),
        "true,true,true,2,1,2,2,true,d800,d800,ffff"
    );
}
#[test]
fn sum_precise_normalize_and_restricted_function_properties() {
    // Math.sumPrecise rounds once (exact sum), String.prototype.normalize follows UAX #15,
    // and Function.prototype.caller/arguments are %ThrowTypeError% accessors (§10.2.4).
    let mut e = engine();
    e.execute(
        "const big = Number.MAX_VALUE;
         const d = Object.getOwnPropertyDescriptor(Function.prototype, 'caller');
         const result = [Math.sumPrecise([1e20, 0.1, -1e20]), Math.sumPrecise([0.1, 0.2, 0.3]),
           Math.sumPrecise([big, big, -big]), Math.sumPrecise([big, big]), Object.is(Math.sumPrecise([]), -0),
           '\\u0041\\u030A'.normalize() === '\\u00C5', '\\uFB01'.normalize('NFKD'),
           '\\u00C5'.normalize('NFD').length, d.get === d.set, Object.isFrozen(d.get)].join();",
    )
    .unwrap();
    assert_eq!(
        e.binding_string("result").unwrap(),
        "0.1,0.6,1.7976931348623157e+308,Infinity,true,true,fi,2,true,true"
    );
    // §12.5: a hashbang is only a comment at the very start of the source text.
    assert!(e.execute("#!/usr/bin/env js\nvar hashbang = 1;").is_ok());
    assert!(e.execute("var x = 1; #!no").is_err());
}
#[test]
fn proper_tail_calls_run_in_constant_depth() {
    // §15.10: calls in tail position (return, ?:, &&, ||, ??, comma, tagged templates)
    // do not grow the call depth, for calls and for [[Construct]] of a strict function.
    let mut e = engine();
    e.execute(
        "'use strict';
         function f(n) { if (n === 0) return 'done'; return n % 2 ? (0, f(n - 1)) : true && f(n - 1); }
         const tag = (s, n) => n === 0 ? 'tag' : tag`${n - 1}`;
         function C(n) { if (n === 0) return {made: 1}; return C(n - 1); }
         const result = [f(100), tag`${100}`, new C(100).made].join();",
    )
    .unwrap();
    assert_eq!(e.binding_string("result").unwrap(), "done,tag,1");
}
