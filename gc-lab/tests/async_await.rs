use gc_lab::{
    script::{parser::parse, Engine},
    Model,
};
fn check(source: &str) {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute(source)
            .unwrap_or_else(|err| panic!("sync {err:?}\n{source}"));
        e.collect();
        e.microtask_checkpoint(1000)
            .unwrap_or_else(|err| panic!("async {err:?}\n{source}"));
        assert!(
            e.unhandled_test_failure().is_none(),
            "{:?}",
            e.unhandled_test_failure()
        );
        assert!(e.take_microtask_errors().is_empty());
        assert!(e.assertion_count() > 0);
        e.release_globals();
        assert_eq!(
            (
                e.live_objects(),
                e.live_roots(),
                e.metadata_objects(),
                e.charged_bytes()
            ),
            (0, 0, 0, 0)
        );
    }
}
#[test]
fn starts_synchronously_and_always_suspends_at_await() {
    check("let order='';async function f(){order=order+'a';const x=await 3;order=order+'c';return x+1;}const p=f();assert.sameValue(order,'a');order=order+'b';p.then(function(v){assert.sameValue(v,4);assert.sameValue(order,'abc');});");
}
#[test]
fn rejected_await_is_caught_and_finally_is_resumable() {
    check("const marker={};async function f(){try{await Promise.reject(marker);assert(false);}catch(error){assert.sameValue(error,marker);return {n:7};}finally{await 1;const garbage=[{a:{}}];}}f().then(function(v){assert.sameValue(v.n,7);});async function g(){try{return 1;}finally{await 2;return 3;}}g().then(function(v){assert.sameValue(v,3);});");
}
#[test]
fn uncaught_throw_rejects_instead_of_throwing_at_call() {
    check("const marker={};async function f(){throw marker;}let p;try{p=f();}catch(e){assert(false);}p.catch(function(e){assert.sameValue(e,marker);});async function g(){await 1;throw marker;}g().catch(function(e){assert.sameValue(e,marker);});");
}
#[test]
fn method_receiver_and_callee_getter_are_saved_before_await_arguments() {
    check("let order='';const o={n:5,get f(){order=order+'g';return function(x){assert.sameValue(this,o);return this.n+x;};}};async function f(){return o.f(await Promise.resolve(3));}const p=f();assert.sameValue(order,'g');o.n=7;p.then(function(v){assert.sameValue(v,10);assert.sameValue(order,'g');});");
}
#[test]
fn reference_key_value_is_saved_but_coercion_waits_for_the_assignment() {
    check("let order='';const object={};const key={toString:function(){order=order+'k';return 'x';}};function getKey(){order=order+'e';return key;}function rhs(){order=order+'r';return 7;}async function f(){object[getKey()]=await rhs();return object.x;}const p=f();assert.sameValue(order,'er');key.toString=function(){order=order+'n';return 'x';};p.then(function(v){assert.sameValue(v,7);assert.sameValue(order,'ern');});");
}
#[test]
fn arrays_objects_and_short_circuit_preserve_effects() {
    check("let n=0;function next(){n=n+1;return n;}async function f(){const a=[next(),await next(),next()];const o={a:next(),b:await next()};const x=false&&await next();const y=true||await next();assert.sameValue(x,false);assert.sameValue(y,true);return a.join()+':'+String(o.a)+':'+String(o.b);}f().then(function(v){assert.sameValue(v,'1,2,3:4:5');assert.sameValue(n,5);});");
}
#[test]
fn async_arrow_captures_this_and_arguments() {
    check("function make(x){return async()=>{await 1;return this.n+arguments[0];};}const o={n:5};const f=make.call(o,3);f.call({n:99}).then(function(v){assert.sameValue(v,8);});assert.throws(TypeError,function(){new f();});const m={async f(){await 1;return this;}};m.f().then(function(v){assert.sameValue(v,m);});");
}
#[test]
fn await_does_not_call_overridden_then() {
    check("const p=Promise.resolve(7);p.then=function(){throw new RangeError();};async function f(){return await p;}f().then(function(v){assert.sameValue(v,7);});");
}
#[test]
fn lexical_scope_and_function_hoisting_survive_suspension() {
    check("async function f(){const x=1;{const x=await 2;assert.sameValue(x,2);await 3;}assert.sameValue(x,1);return nested();function nested(){return 7;}}f().then(function(v){assert.sameValue(v,7);});");
}
#[test]
fn parser_rejects_await_outside_async_and_across_ordinary_function_boundary() {
    for source in [
        "await 1;",
        "function f(){await 1;}",
        "async function f(){function g(){await 1;}}",
        "async function f(await){}",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
}

#[test]
fn nullish_property_access_does_not_coerce_the_key() {
    check("let keys=0;let rhs=0;const key={toString:function(){keys=keys+1;return 'x';}};assert.throws(TypeError,function(){null[key];});async function f(){try{null[key]=await next();assert(false);}catch(error){assert.sameValue(error.constructor,TypeError);assert.sameValue(rhs,1);assert.sameValue(keys,0);}}function next(){rhs=rhs+1;return 1;}f();");
}
