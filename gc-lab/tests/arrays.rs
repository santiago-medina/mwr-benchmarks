use gc_lab::{script::Engine, Model};
fn check(source: &str) {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute(source)
            .unwrap_or_else(|err| panic!("{err:?}\n{source}"));
        assert!(e.assertion_count() > 0);
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
fn indexof_zero_length_does_not_coerce_from_index() {
    check("let calls=0;const from={valueOf:function(){calls++;throw new RangeError();}};assert.sameValue([].indexOf(1,from),-1);assert.sameValue(calls,0);const a=[3];assert.sameValue(a.indexOf(3,0),0);assert.sameValue(a.indexOf(3,1),-1);");
}
#[test]
fn literal_holes_and_boundaries() {
    check("const a=[1,,3,];assert.sameValue(a.length,3);assert.sameValue(a.hasOwnProperty('1'),false);assert.sameValue(a[1],undefined);a[5]=6;assert.sameValue(a.length,6);a['4294967295']=7;assert.sameValue(a.length,6);assert.sameValue(Array.isArray(a),true);assert.sameValue(Array.isArray({}),false);assert.sameValue(Array.prototype.length,0);");
}
#[test]
fn shrinking_length_has_specified_partial_effects() {
    check("const a=[0,1,2,3,4];Object.defineProperty(a,'2',{configurable:false});assert.sameValue(Reflect.defineProperty(a,'length',{value:1,writable:false}),false);assert.sameValue(a.length,3);assert.sameValue(a.hasOwnProperty('3'),false);assert.sameValue(a.hasOwnProperty('4'),false);assert.sameValue(a[2],2);assert.sameValue(Object.getOwnPropertyDescriptor(a,'length').writable,false);assert.sameValue(Reflect.defineProperty(a,'3',{value:9}),false);");
}
#[test]
fn length_range_coercion_and_nonwritable() {
    check("let calls=0;const n={valueOf:function(){calls=calls+1;return 1;}};const a=[1,2];Object.defineProperty(a,'length',{value:n});assert.sameValue(calls,2);assert.sameValue(a.length,1);assert.throws(RangeError,function(){a.length=1.5;});assert.throws(RangeError,function(){a.length=-1;});Object.defineProperty(a,'length',{writable:false});assert.sameValue(Reflect.defineProperty(a,'1',{value:3}),false);a[0]=8;assert.sameValue(a[0],8);assert.throws(TypeError,function(){a.push();});");
}
#[test]
fn constructors_and_freeze() {
    check("const a=new Array(3);assert.sameValue(a.length,3);assert.sameValue(a.hasOwnProperty('0'),false);const b=Array(1,2);assert.sameValue(b.length,2);assert.sameValue(Array('x')[0],'x');assert.throws(RangeError,function(){new Array(NaN);});Object.freeze(b);assert(Object.isFrozen(b));assert.throws(TypeError,function(){b.push(3);});assert.throws(TypeError,function(){b.pop();});");
}
#[test]
fn generic_push_pop_and_holes() {
    check("const o={length:1,0:'a'};assert.sameValue(Array.prototype.push.call(o,'b','c'),3);assert.sameValue(o[2],'c');assert.sameValue(Array.prototype.pop.call(o),'c');assert.sameValue(o.length,2);const a=[1,,];assert.sameValue(a.pop(),undefined);assert.sameValue(a.length,1);assert.sameValue(a.pop(),1);assert.sameValue(a.pop(),undefined);");
}
#[test]
fn join_and_cycle_detection() {
    check("const a=[1,,null,undefined,[2,3]];assert.sameValue(a.join('|'),'1||||2,3');assert.sameValue(String([1,2]),'1,2');const b=[];b.push(b);assert.sameValue(b.join(),'');assert.throws(TypeError,function(){[Symbol()].join();});");
}
#[test]
fn map_filter_snapshot_holes_and_receivers() {
    check("const a=[1,,3];let count=0;const receiver={factor:2};const b=a.map(function(v,i,obj){assert.sameValue(obj,a);count=count+1;if(i===0){a.push(9);}return v*this.factor;},receiver);assert.sameValue(count,2);assert.sameValue(b.length,3);assert.sameValue(b.hasOwnProperty('1'),false);assert.sameValue(b[2],6);const c=a.filter(function(v){return v>1;});assert.sameValue(c.join(), '3,9');assert(a.some(function(v){return v===9;}));assert(a.every(function(v){return v>0;}));");
}
#[test]
fn symbol_keys_do_not_alias_strings() {
    check("const s=Symbol();const t=Symbol();const o={};o[s]=3;o[t]=4;o['Symbol()']=5;assert.sameValue(o[s],3);assert.sameValue(o[t],4);assert.sameValue(Object.getOwnPropertyDescriptor(o,s).value,3);assert.sameValue(typeof s,'symbol');assert.notSameValue(s,t);assert.throws(TypeError,function(){return ''+s;});assert.sameValue(delete o[s],true);assert.sameValue(o[s],undefined);");
}
#[test]
fn iterators_observe_growth_and_stay_done() {
    check("const a=[1,,];const i=a.values();assert.sameValue(a[Symbol.iterator],a.values);assert.sameValue(i[Symbol.iterator](),i);assert.sameValue(i.next().value,1);a.push(3);assert.sameValue(i.next().value,undefined);assert.sameValue(i.next().value,3);assert.sameValue(i.next().done,true);a.push(4);assert.sameValue(i.next().done,true);const e=a.entries().next();assert.sameValue(e.value.join(), '0,1');assert.sameValue(a.keys().next().value,0);");
}
#[test]
fn iterator_getter_error_advances_index() {
    check("const a=[1,2];Object.defineProperty(a,'0',{get:function(){throw new RangeError();}});const i=a.values();assert.throws(RangeError,function(){i.next();});assert.sameValue(i.next().value,2);");
}
#[test]
fn for_of_binds_fresh_lexical_environments() {
    check("let sum=0;const callbacks=[];for(let x of [1,2,3]){sum=sum+x;callbacks.push(()=>x);}assert.sameValue(sum,6);assert.sameValue(callbacks[0](),1);assert.sameValue(callbacks[2](),3);for(var last of [4,5]){}assert.sameValue(last,5);assert.throws(ReferenceError,function(){const x=[1];for(const x of x){}});");
}
#[test]
fn for_of_closes_on_break_return_and_throw() {
    check("let closed=0;const iterator={next:function(){return {value:{n:7},done:false};},return:function(){closed=closed+1;return {};}};const source={};source[Symbol.iterator]=function(){return iterator;};for(const x of source){assert.sameValue(x.n,7);break;}assert.sameValue(closed,1);function f(){for(const x of source){return x;}}assert.sameValue(f().n,7);assert.sameValue(closed,2);const marker={};try{for(const x of source){throw marker;}}catch(e){assert.sameValue(e,marker);}assert.sameValue(closed,3);");
}
#[test]
fn close_preserves_original_throw_and_finally_completions() {
    check("let closed=0;const i={next:function(){return {value:1,done:false};},return:function(){closed=closed+1;throw new RangeError();}};const o={};o[Symbol.iterator]=()=>i;const marker={};try{for(const x of o){throw marker;}}catch(e){assert.sameValue(e,marker);}assert.throws(RangeError,function(){for(const x of o){break;}});assert.sameValue(closed,2);let sum=0;for(const x of [1,2,3]){try{if(x===2){continue;}sum=sum+x;}finally{sum=sum+10;}}assert.sameValue(sum,34);");
}
#[test]
fn iterator_next_failure_does_not_close() {
    check("let closed=0;const i={next:function(){throw new RangeError();},return:function(){closed=closed+1;return {};}};const o={};o[Symbol.iterator]=()=>i;assert.throws(RangeError,function(){for(const x of o){}});assert.sameValue(closed,0);");
}
#[test]
fn species_and_generic_map() {
    check("const a=[1,2];const c={};c[Symbol.species]=function(n){this.length=n;};a.constructor=c;const b=a.map(function(x){return x*2;});assert.sameValue(Array.isArray(b),false);assert.sameValue(b[1],4);assert.sameValue(b.length,2);const generic={0:3,length:1,get constructor(){throw new RangeError();}};assert.sameValue(Array.prototype.map.call(generic,function(x){return x+1;})[0],4);assert.sameValue(Array[Symbol.species],Array);");
}
#[test]
fn reflection_preserves_string_symbol_and_index_order() {
    check("const s=Symbol();const o={b:1,2:2,1:3};o[s]=4;assert.sameValue(Object.keys(o).join(),'1,2,b');assert.sameValue(Object.getOwnPropertySymbols(o)[0],s);const keys=Reflect.ownKeys(o);assert.sameValue(keys.length,4);assert.sameValue(keys[3],s);assert.sameValue(Object.keys('ab').join(),'0,1');");
}
#[test]
fn iterator_is_the_only_owner_and_exhaustion_releases_its_target() {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute("const iterator=[{n:7}].values();function consume(){assert.sameValue(iterator.next().value.n,7);assert.sameValue(iterator.next().done,true);}").unwrap();
        e.collect();
        let before = e.live_objects();
        e.call_global("consume").unwrap();
        e.collect();
        assert!(e.live_objects() + 2 <= before);
        assert_eq!(e.live_roots(), 1);
        e.release_globals();
        assert_eq!(
            (e.live_objects(), e.metadata_objects(), e.live_roots()),
            (0, 0, 0)
        );
    }
}
#[test]
fn pending_return_and_throw_arrays_survive_allocating_iterator_close() {
    check("const i={next:function(){return {value:1,done:false};},return:function(){const noise=[{a:{}},{b:{}}];return {};}};const source={};source[Symbol.iterator]=()=>i;function f(){for(const x of source){return [{n:7}];}}assert.sameValue(f()[0].n,7);function g(){for(const x of source){throw [{n:8}];}}try{g();assert(false);}catch(e){assert.sameValue(e[0].n,8);}");
}
#[test]
fn quota_failure_cannot_publish_an_index_without_its_length() {
    let mut successes = 0;
    let mut failures = 0;
    for extra in [0, 64, 128, 256, 512, 1024, 2048] {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.enable_test_harness();
        e.execute("const a=[];const value={n:7};function grow(){a[4]=value;}function unchanged(){assert.sameValue(a.length,0);assert.sameValue(a.hasOwnProperty('4'),false);}function changed(){assert.sameValue(a.length,5);assert.sameValue(a[4],value);}").unwrap();
        let baseline = e.charged_bytes();
        e.set_budget(Some(baseline + extra));
        let result = e.call_global("grow");
        e.set_budget(None);
        if result.is_ok() {
            successes += 1;
            e.call_global("changed").unwrap();
        } else {
            failures += 1;
            e.call_global("unchanged").unwrap();
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
    assert!(successes > 0 && failures > 0);
}
#[test]
fn iterator_close_does_not_turn_a_quota_failure_into_a_catchable_js_throw() {
    let mut checked = 0;
    for extra in (0..2048).step_by(64).chain([4096, 8192]) {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.execute("const marker={};const state={started:false,finished:false,caught:false};const i={next:function(){return {value:1,done:false};},return:function(){state.started=true;const noise=[{a:{}},{b:{}},{c:{}}];state.finished=true;return {};}};const source={};source[Symbol.iterator]=()=>i;function work(){try{for(const x of source){throw marker;}}catch(e){state.caught=true;}}function verify(){if(state.started&&!state.finished&&state.caught){throw 'quota was hidden';}}").unwrap();
        let base = e.charged_bytes();
        e.set_budget(Some(base + extra));
        let _ = e.call_global("work");
        e.set_budget(None);
        e.call_global("verify").unwrap();
        checked += 1;
        e.release_globals();
        assert_eq!(e.live_roots(), 0);
    }
    assert!(checked > 0);
}
#[test]
fn finally_return_does_not_hide_a_terminal_allocation_failure() {
    let mut failed = 0;
    let mut succeeded = 0;
    for caught in [false, true] {
        for extra in (0..2048).step_by(64).chain([4096, 8192]) {
            let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
            let program="const state={started:false,finished:false};function work(){try{state.started=true;const noise=[{a:{}},{b:{}},{c:{}}];state.finished=true;}finally{return 1;}}function verify(){if(state.started&&!state.finished){throw 'allocation was hidden';}}";
            let program = if caught {
                program.replace(
                    "try{state.started",
                    "try{throw 1;}catch(error){state.started",
                )
            } else {
                program.into()
            };
            e.execute(&program).unwrap();
            let base = e.charged_bytes();
            e.set_budget(Some(base + extra));
            let result = e.call_global("work");
            e.set_budget(None);
            if result.is_ok() {
                succeeded += 1;
                e.call_global("verify").unwrap();
            } else {
                failed += 1;
            }
            e.release_globals();
            assert_eq!((e.live_objects(), e.live_roots()), (0, 0));
        }
    }
    assert!(failed > 0 && succeeded > 0);
}
