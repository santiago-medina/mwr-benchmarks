use gc_lab::{
    script::{
        parser::parse_module, Engine, ModuleLoad, ModuleSource, ModuleStatus, Referrer, ScriptError,
    },
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
                "if(globalThis.runs===undefined){globalThis.runs=0;}globalThis.runs=globalThis.runs+1;export function count(){return globalThis.runs;}",
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
        "import {x} from 'a';{var x=1;}",
        "function f(){}var f;",
        "import {x} from 'a';function x(){}",
    ] {
        assert!(parse_module(s).is_err(), "{s}");
    }
    assert!(parse_module("export const x=await 1;").is_ok());
}

#[test]
fn independent_siblings_execute_while_an_async_dependency_waits() {
    run(vec![source("main","import {log} from './shared';import './a';import './b';assert.sameValue(log.join(','),'start,b,end');",&[("./shared","shared"),("./a","a"),("./b","b")]),source("shared","export const log=[];",&[]),source("a","import {log} from './shared';log.push('start');await 1;log.push('end');",&[("./shared","shared")]),source("b","import {log} from './shared';log.push('b');",&[("./shared","shared")])],ModuleStatus::Evaluated);
}
#[test]
fn default_function_declarations_are_hoisted_through_cycles() {
    run(vec![source("main","import {read} from './dep';export default function(){return 7;}assert.sameValue(read(),7);",&[("./dep","dep")]),source("dep","import get from './main';assert.sameValue(get(),7);assert.sameValue(get.name,'default');export function read(){return get();}",&[("./main","main")])],ModuleStatus::Evaluated);
}
#[test]
fn rejection_is_cached_and_propagated_to_the_importer() {
    run(
        vec![
            source("main", "import './dep';assert(false);", &[("./dep", "dep")]),
            source("dep", "await 1;throw new Error('boom');", &[]),
        ],
        ModuleStatus::Errored("Error: boom".into()),
    );
}

#[test]
fn self_import_does_not_wait_for_its_own_completion() {
    run(
        vec![source(
            "main",
            "import {n as same} from './self';export const n=7;await 1;assert.sameValue(same,7);",
            &[("./self", "main")],
        )],
        ModuleStatus::Evaluated,
    );
}
#[test]
fn syntactic_top_level_await_remains_asynchronous_when_not_reached() {
    run(vec![source("main","import {log} from './trace';import './a';assert.sameValue(log.join(','),'q');",&[("./trace","trace"),("./a","a")]),source("trace","export const log=[];",&[]),source("a","import {log} from './trace';Promise.resolve().then(function(){log.push('q');});if(false){await 1;}",&[("./trace","trace")])],ModuleStatus::Evaluated);
}
#[test]
fn rejection_before_await_does_not_skip_independent_siblings() {
    run(vec![source("main","import './a';import './b';assert(false);",&[("./a","a"),("./b","b")]),source("trace","export const log=[];",&[]),source("a","import {log} from './trace';Promise.resolve().then(function(){assert.sameValue(log.join(','),'b');});throw new Error('boom');await 1;",&[("./trace","trace")]),source("b","import {log} from './trace';log.push('b');",&[("./trace","trace")])],ModuleStatus::Errored("Error: boom".into()));
}

fn javascript(url: &str, source: &str) -> ModuleLoad {
    ModuleLoad::Source {
        url: url.into(),
        module_type: "javascript".into(),
        source: source.into(),
        host_defined: 0,
    }
}
#[test]
fn pull_host_rejects_duplicate_and_unknown_finishes() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute_module("main", "import {x} from './dep';globalThis.seen=x;", 7)
        .unwrap();
    let requests = e.take_module_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(e.pending_module_requests(), 1);
    assert_eq!(
        requests[0].referrer,
        Referrer::Module {
            url: "main".into(),
            host_defined: 7
        }
    );
    assert_eq!(requests[0].specifier, "./dep");
    let id = requests[0].id;
    e.finish_module_request(id, javascript("dep", "export const x=1;"))
        .unwrap();
    assert_eq!(e.module_status("main").unwrap(), ModuleStatus::Evaluated);
    assert_eq!(e.pending_module_requests(), 0);
    for late in [id, id + 100] {
        assert!(matches!(
            e.finish_module_request(late, javascript("dep", "export const x=2;")),
            Err(ScriptError::Host(_))
        ));
    }
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn pull_host_teardown_invalidates_pending_requests() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute_module("main", "import './never';", 0).unwrap();
    let id = e.take_module_requests()[0].id;
    // A request that is never answered stays visible to the host.
    e.collect();
    assert_eq!(e.pending_module_requests(), 1);
    e.release_globals();
    assert_eq!(e.pending_module_requests(), 0);
    assert!(matches!(
        e.finish_module_request(id, javascript("never", "")),
        Err(ScriptError::Host(_))
    ));
    assert_eq!((e.live_objects(), e.live_roots()), (0, 0));
}
#[test]
fn pull_host_errors_and_shared_requests() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute_module("main", "import './a';import './b';", 0)
        .unwrap();
    let r = e.take_module_requests();
    assert_eq!(r.len(), 2);
    e.finish_module_request(r[0].id, javascript("a", "export {};"))
        .unwrap();
    let err = e.finish_module_request(
        r[1].id,
        ModuleLoad::Error {
            kind: "TypeError".into(),
            message: "fetch failed".into(),
        },
    );
    assert!(matches!(err, Err(ScriptError::Thrown(ex)) if ex.name.as_deref()==Some("TypeError")));
    // A module key reached by two specifiers is one record.
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute_module(
        "main",
        "import {n} from './x';import {n as m} from 'x';if(n!==1||m!==1)throw 0;",
        0,
    )
    .unwrap();
    for r in e.take_module_requests() {
        e.finish_module_request(r.id, javascript("x", "export let n=0;n++;"))
            .unwrap();
    }
    assert_eq!(e.module_status("main").unwrap(), ModuleStatus::Evaluated);
    assert_eq!(e.loaded_modules().len(), 2);
    // A load whose source has a syntax error fails the graph with that error.
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute_module("main", "import './bad';", 0).unwrap();
    let id = e.take_module_requests()[0].id;
    assert!(matches!(
        e.finish_module_request(id, javascript("bad", "let let;")),
        Err(ScriptError::Syntax(_))
    ));
}

#[test]
fn namespace_objects_follow_the_exotic_object_rules() {
    // §10.4.6: sorted keys, live values, TDZ ReferenceError, frozen shape, "Module" tag.
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute_module(
        "main",
        "import * as ns from './x';
         import * as again from './x';
         if (ns !== again) throw 'identity';
         if (Object.getPrototypeOf(ns) !== null) throw 'proto';
         if (Object.isExtensible(ns)) throw 'extensible';
         if (Object.prototype.toString.call(ns) !== '[object Module]') throw 'tag';
         if (Object.keys(ns).join() !== 'b,bump,c') throw 'keys ' + Object.keys(ns).join();
         if (ns.b !== 1) throw 'b';
         let d = Object.getOwnPropertyDescriptor(ns, 'b');
         if (!d.writable || !d.enumerable || d.configurable) throw 'desc';
         if (Reflect.set(ns, 'b', 2)) throw 'set';
         if (Reflect.deleteProperty(ns, 'b')) throw 'delete b';
         if (!Reflect.deleteProperty(ns, 'nope')) throw 'delete nope';
         if (!('c' in ns) || 'nope' in ns) throw 'has';
         if (Reflect.defineProperty(ns, 'b', {value: 9})) throw 'define other value';
         if (!Reflect.defineProperty(ns, 'b', {value: 1})) throw 'define same value';
         ns.bump(); if (ns.b !== 2) throw 'live';
         export let late = 1;",
        0,
    )
    .unwrap();
    for r in e.take_module_requests() {
        e.finish_module_request(
            r.id,
            javascript(
                "x",
                "import * as m from './main';
                 let tdz = false;
                 try { m.late; } catch (err) { tdz = err instanceof ReferenceError; }
                 if (!tdz) throw 'tdz';
                 export let b = 1; export function c() { b++; } export {c as bump};",
            ),
        )
        .unwrap();
    }
    for r in e.take_module_requests() {
        e.finish_module_request(r.id, javascript("main", "")).ok();
    }
    assert_eq!(e.module_status("main").unwrap(), ModuleStatus::Evaluated);
    assert_eq!(e.module_status("x").unwrap(), ModuleStatus::Evaluated);
}
