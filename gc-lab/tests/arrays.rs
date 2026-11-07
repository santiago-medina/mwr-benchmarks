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
