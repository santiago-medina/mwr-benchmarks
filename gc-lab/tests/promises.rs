use gc_lab::{script::Engine, Model};
fn check(source: &str) {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute(source)
            .unwrap_or_else(|err| panic!("script: {err:?}\n{source}"));
        e.collect();
        e.microtask_checkpoint(1000)
            .unwrap_or_else(|err| panic!("jobs: {err:?}\n{source}"));
        assert!(e.assertion_count() > 0);
        assert!(e.take_microtask_errors().is_empty());
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
fn executor_is_sync_reactions_are_deferred_fifo() {
    check("let order='';new Promise(function(resolve){order=order+'e';resolve(1);}).then(function(v){assert.sameValue(v,1);order=order+'a';}).then(function(){assert.sameValue(order,'esab');});queueMicrotask(function(){order=order+'b';});order=order+'s';assert.sameValue(order,'es');");
}
#[test]
fn chains_assimilate_results_and_propagate_rejections() {
    check("Promise.resolve(1).then(function(v){return v+1;}).then(function(v){assert.sameValue(v,2);throw new RangeError('bad');}).catch(function(e){assert.sameValue(e.constructor,RangeError);return 3;}).then(function(v){assert.sameValue(v,3);});const marker={};Promise.reject(marker).then().catch(function(e){assert.sameValue(e,marker);});");
}
#[test]
fn resolving_functions_share_once_only_cell() {
    check("let resolve;let reject;const p=new Promise(function(a,b){resolve=a;reject=b;});resolve(7);reject(8);resolve(9);p.then(function(v){assert.sameValue(v,7);});new Promise(function(resolve){resolve(4);throw 5;}).then(function(v){assert.sameValue(v,4);});");
}
#[test]
fn then_getter_is_sync_but_then_call_is_a_job() {
    check("let order='';const o={get then(){order=order+'g';return function(resolve,reject){order=order+'t';resolve(8);reject(9);throw 10;};}};Promise.resolve(o).then(function(v){assert.sameValue(v,8);assert.sameValue(order,'gst');});order=order+'s';assert.sameValue(order,'gs');");
}
#[test]
fn thenable_getter_throws_and_self_resolution_reject() {
    check("const marker={};Promise.resolve({get then(){throw marker;}}).catch(function(e){assert.sameValue(e,marker);});let resolve;const p=new Promise(function(r){resolve=r;});resolve(p);p.catch(function(e){assert.sameValue(e.constructor,TypeError);});let q;q=Promise.resolve().then(function(){return q;});q.catch(function(e){assert.sameValue(e.constructor,TypeError);});");
}
#[test]
fn fulfilled_identity_and_constructor_species() {
    check("const p=Promise.resolve(1);assert.sameValue(Promise.resolve(p),p);assert.sameValue(Promise[Symbol.species],Promise);const c={};c[Symbol.species]=function(exec){exec(function(v){assert.sameValue(v,3);},function(){assert(false);});this.marker=7;};p.constructor=c;const result=p.then(function(v){return v+2;});assert.sameValue(result.marker,7);");
}
#[test]
fn finally_waits_and_preserves_or_overrides_completion() {
    check("Promise.resolve(7).finally(function(){return Promise.resolve(9);}).then(function(v){assert.sameValue(v,7);});const marker={};Promise.reject(marker).finally(function(){}).catch(function(e){assert.sameValue(e,marker);});Promise.resolve(1).finally(function(){throw marker;}).catch(function(e){assert.sameValue(e,marker);});Promise.resolve(2).finally(function(){return Promise.reject(marker);}).catch(function(e){assert.sameValue(e,marker);});");
}
#[test]
fn catch_and_finally_use_overridden_then_generically() {
    check("const o={then:function(a,b){assert.sameValue(a,undefined);assert.sameValue(b,3);return 9;}};assert.sameValue(Promise.prototype.catch.call(o,3),9);const f={then:function(a,b){assert.sameValue(a,5);assert.sameValue(b,5);return 8;}};assert.sameValue(Promise.prototype.finally.call(f,5),8);assert.throws(TypeError,function(){Promise(function(){});});assert.throws(TypeError,function(){new Promise(1);});assert.throws(TypeError,function(){Promise.prototype.then.call({});});");
}
#[test]
fn queue_is_a_root_and_cancelled_tokens_are_stale() {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute("Promise.resolve({n:7}).then(function(v){assert.sameValue(v.n,7);});")
            .unwrap();
        assert_eq!(e.pending_microtasks(), 1);
        e.collect();
        e.microtask_checkpoint(20).unwrap();
        assert_eq!(e.assertion_count(), 1);
        e.release_globals();
        assert_eq!(e.live_objects(), 0);
        e.execute("queueMicrotask(function(){throw 1;});").unwrap();
        e.cancel_microtasks();
        assert!(!e.run_one_microtask(Some(1)).unwrap());
        assert_eq!(e.pending_microtasks(), 0);
        e.release_globals();
        assert_eq!(e.live_roots(), 0);
    }
}
#[test]
fn callback_error_reports_and_later_jobs_still_run() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.enable_test_harness();
    e.execute("queueMicrotask(function(){throw new RangeError('reported');});queueMicrotask(function(){assert(true);});").unwrap();
    assert_eq!(e.microtask_checkpoint(10).unwrap(), 2);
    assert_eq!(e.assertion_count(), 1);
    assert_eq!(e.take_microtask_errors().len(), 1);
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn runaway_jobs_are_bounded_and_realm_release_cancels() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute("function f(){queueMicrotask(f);}queueMicrotask(f);")
        .unwrap();
    assert!(e.microtask_checkpoint(20).is_err());
    assert_eq!(e.pending_microtasks(), 0);
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
    e.execute("Promise.resolve().then(function(){throw 7;});")
        .unwrap();
    e.release_globals();
    assert_eq!(e.pending_microtasks(), 0);
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn quotas_in_jobs_are_terminal_and_do_not_become_js_rejections() {
    let mut errors = 0;
    let mut successes = 0;
    for extra in [0, 64, 128, 256, 512, 1024, 2048, 4096] {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.enable_test_harness();
        e.execute("Promise.resolve().then(function(){return [{a:{}},{b:{}}];}).then(function(v){assert.sameValue(v.length,2);});").unwrap();
        let baseline = e.charged_bytes();
        e.set_budget(Some(baseline + extra));
        let result = e.microtask_checkpoint(100);
        e.set_budget(None);
        if result.is_ok() {
            successes += 1;
        } else {
            errors += 1;
            assert_eq!(e.pending_microtasks(), 0);
        }
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
    assert!(errors > 0 && successes > 0);
}
#[test]
fn reflected_construction_and_builtin_property_order() {
    check("const bound=function(){}.bind();Object.defineProperty(bound,'prototype',{get:function(){throw new Test262Error();}});assert.throws(Test262Error,function(){Reflect.construct(Promise,[function(){}],bound);});const keys=Object.getOwnPropertyNames(Promise);assert.sameValue(keys.indexOf('name'),keys.indexOf('length')+1);const C=function(){};const custom={};C.prototype=custom;const p=Reflect.construct(Promise,[function(r){r(8);}],C);assert.sameValue(Object.getPrototypeOf(p),custom);Promise.prototype.then.call(p,function(v){assert.sameValue(v,8);});");
}
#[test]
fn queue_capacity_failure_cancels_roots() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.set_microtask_queue_limit(2);
    assert!(e
        .execute("function f(){}queueMicrotask(f);queueMicrotask(f);queueMicrotask(f);")
        .is_err());
    assert_eq!(
        (
            e.pending_microtasks(),
            e.live_objects(),
            e.metadata_objects(),
            e.live_roots()
        ),
        (0, 0, 0, 0)
    );
}
#[test]
fn done_is_latched_and_assertions_in_reactions_are_not_false_passes() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.enable_test_harness();
    e.execute("Promise.resolve().then(function(){$DONE();$DONE('second');});")
        .unwrap();
    e.microtask_checkpoint(100).unwrap();
    assert_eq!(e.async_test_state().0, 2);
    assert!(e.async_test_state().1.is_some());
    e.release_globals();
    e.execute("Promise.resolve().then(function(){assert(false);});")
        .unwrap();
    e.microtask_checkpoint(100).unwrap();
    assert!(e.unhandled_test_failure().is_some());
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
#[test]
fn a_terminal_public_call_cancels_previously_queued_jobs() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute("queueMicrotask(function(){});function fail(){const a={};}")
        .unwrap();
    e.set_budget(Some(e.charged_bytes()));
    assert!(e.call_global("fail").is_err());
    assert_eq!(e.pending_microtasks(), 0);
    e.set_budget(None);
    e.release_globals();
    assert_eq!(e.live_objects(), 0);
}
