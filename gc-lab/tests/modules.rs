use gc_lab::{
    script::{parser::parse_module, Engine, ModuleSource, ModuleStatus},
    Model,
};
fn source(url: &str, body: &str, deps: &[(&str, &str)]) -> ModuleSource {
    ModuleSource {
        url: url.into(),
        source: body.into(),
        resolutions: deps
            .iter()
            .map(|(s, u)| (s.to_string(), u.to_string()))
            .collect(),
    }
}
fn run(sources: Vec<ModuleSource>, expected: ModuleStatus) {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute_modules("main", &sources).unwrap();
        e.collect();
        e.microtask_checkpoint(1000).unwrap();
        assert_eq!(e.module_status("main").unwrap(), expected);
        assert!(e.take_microtask_errors().is_empty());
        assert!(
            e.unhandled_test_failure().is_none(),
            "{:?}",
            e.unhandled_test_failure()
        );
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
fn imports_are_live_and_readonly() {
    run(vec![source("counter","export let count=0;export function bump(){count=count+1;}",&[]),source("main","import {count,bump} from './counter';assert.sameValue(count,0);bump();assert.sameValue(count,1);assert.throws(TypeError,function(){count=9;});assert.sameValue(this,undefined);assert.sameValue(globalThis.count,undefined);",&[("./counter","counter")])],ModuleStatus::Evaluated);
}
#[test]
fn cyclic_functions_are_instantiated_before_evaluation() {
    run(vec![source("main","import {read} from './b';export const value=7;export function fromA(){return 3;}assert.sameValue(read(),7);",&[("./b","b")]),source("b","import {value,fromA} from './a';assert.sameValue(fromA(),3);export function read(){return value;}",&[("./a","main")])],ModuleStatus::Evaluated);
}
#[test]
fn reexports_and_default_imports_resolve_to_original_bindings() {
    run(vec![source("main","import value,{n,change} from './barrel';assert.sameValue(value,7);assert.sameValue(n,1);change();assert.sameValue(n,2);",&[("./barrel","barrel")]),source("barrel","export {default,n,change} from './dep';",&[("./dep","dep")]),source("dep","export default 7;export let n=1;export function change(){n=2;}",&[])],ModuleStatus::Evaluated);
}
#[test]
fn diamond_graph_executes_dependency_once() {
    run(
        vec![
            source(
                "main",
                "import {count} from './a';import './b';assert.sameValue(count(),1);",
                &[("./a", "a"), ("./b", "b")],
            ),
            source(
                "a",
                "export {count} from './shared';",
                &[("./shared", "shared")],
            ),
            source("b", "import './shared';", &[("./shared", "shared")]),
            source(
                "shared",
                "globalThis.runs=1;export function count(){return globalThis.runs;}",
                &[],
            ),
        ],
        ModuleStatus::Evaluated,
    );
}
#[test]
fn top_level_await_holds_parent_and_preserves_live_exports() {
    run(vec![source("main","import {n} from './dep';assert.sameValue(n,7);const got=await n;assert.sameValue(got,7);",&[("./dep","dep")]),source("dep","export let n=1;await Promise.resolve();n=7;",&[])],ModuleStatus::Evaluated);
}
#[test]
fn asynchronous_cycle_waits_for_descendant_execution() {
    run(vec![source("main","import {n} from './b';export function ping(){return 3;}assert.sameValue(n,7);await 1;",&[("./b","b")]),source("b","import {ping} from './a';assert.sameValue(ping(),3);export let n=1;await 1;n=7;",&[("./a","main")])],ModuleStatus::Evaluated);
}
#[test]
fn missing_exports_fail_linking_before_any_effect() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    let result = e.execute_modules(
        "main",
        &[
            source(
                "main",
                "import {missing} from './dep';assert(false);",
                &[("./dep", "dep")],
            ),
            source("dep", "throw new Error('should not execute');", &[]),
        ],
    );
    assert!(result.is_err());
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn parser_rejects_import_collisions_and_duplicate_exports() {
    for s in [
        "import {x} from 'a';let x=1;",
        "import {x} from 'a';import {y as x} from 'b';",
        "export const x=1;export {x};",
        "return 1;",
        "import {x} from 'a';function x(){}",
    ] {
        assert!(parse_module(s).is_err(), "{s}");
    }
    assert!(parse_module("export const x=await 1;").is_ok());
}
