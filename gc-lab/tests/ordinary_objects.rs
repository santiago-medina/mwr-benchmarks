use gc_lab::{script::Engine, Model};
fn check(source: &str) {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute(source)
            .unwrap_or_else(|err| panic!("{err:?}\n{source}"));
        assert!(e.assertion_count() > 0);
        e.collect();
        assert_eq!(e.live_roots(), 1);
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
fn identity_and_numbers() {
    check("const a={x:2};const b=a;b.x=b.x+3;assert.sameValue(a.x,5);assert.notSameValue(a,{});assert.sameValue(NaN,NaN);assert.notSameValue(0,-0);");
}
#[test]
fn descriptor_defaults_and_same_value() {
    check("const o={};Object.defineProperty(o,'x',{value:NaN});const d=Object.getOwnPropertyDescriptor(o,'x');assert.sameValue(d.writable,false);assert.sameValue(d.enumerable,false);assert.sameValue(d.configurable,false);assert(Reflect.defineProperty(o,'x',{value:NaN}));assert.sameValue(Reflect.defineProperty(o,'x',{value:0}),false);assert.throws(TypeError,function(){o.x=1;});");
}
#[test]
fn writable_can_only_be_disabled() {
    check("const o={};Object.defineProperty(o,'x',{value:1,writable:true});assert(Reflect.defineProperty(o,'x',{value:2,writable:false}));assert.sameValue(Reflect.defineProperty(o,'x',{writable:true}),false);assert.sameValue(o.x,2);");
}
#[test]
fn inherited_accessors_use_actual_receiver() {
    check("const p={get x(){return this.y;},set x(v){this.y=v;}};const o=Object.create(p);o.x=7;assert.sameValue(o.y,7);assert.sameValue(o.x,7);assert.sameValue(p.y,undefined);const r={y:9};assert.sameValue(Reflect.get(p,'x',r),9);assert(Reflect.set(p,'x',11,r));assert.sameValue(r.y,11);");
}
#[test]
fn receiver_descriptors_control_reflect_set() {
    check("const p={x:1};const r={};Object.defineProperty(r,'x',{value:2});assert.sameValue(Reflect.set(p,'x',3,r),false);assert.sameValue(r.x,2);const a={get x(){return 5;}};assert.sameValue(Reflect.set(p,'x',3,a),false);");
}
#[test]
fn define_can_shadow_inherited_nonwritable() {
    check("const p={};Object.defineProperty(p,'x',{value:1});const o=Object.create(p);assert.throws(TypeError,function(){o.x=2;});Object.defineProperty(o,'x',{value:3});assert.sameValue(o.x,3);assert.sameValue(p.x,1);");
}
#[test]
fn prototype_cycles_and_prevent_extensions() {
    check("const a={};const b=Object.create(a);assert.sameValue(Reflect.setPrototypeOf(a,b),false);Object.preventExtensions(b);assert(Reflect.setPrototypeOf(b,a));assert.sameValue(Reflect.setPrototypeOf(b,null),false);assert.throws(TypeError,function(){Object.setPrototypeOf(b,null);});assert.sameValue(Reflect.defineProperty(b,'x',{value:1}),false);");
}
#[test]
fn descriptor_accessor_identity_and_mixed_rejection() {
    check("const f=function(){return 4;};const o={};Object.defineProperty(o,'x',{get:f});assert.sameValue(Object.getOwnPropertyDescriptor(o,'x').get,f);assert(Reflect.defineProperty(o,'x',{get:f}));assert.sameValue(Reflect.defineProperty(o,'x',{get:function(){}}),false);assert.throws(TypeError,function(){Object.defineProperty(o,'z',{value:1,get:f});});");
}
#[test]
fn descriptor_inherited_fields_and_order() {
    check("let order='';const p={get enumerable(){order=order+'e';return true;},get configurable(){order=order+'c';return true;},get value(){order=order+'v';return 8;},get writable(){order=order+'w';return true;}};const o={};Object.defineProperty(o,'x',Object.create(p));assert.sameValue(order,'ecvw');assert.sameValue(o.x,8);");
}
#[test]
fn define_properties_converts_all_before_mutation() {
    check("const o={};const marker={};assert.throws(TypeError,function(){Object.defineProperties(o,{a:{value:1},b:{get:3}});});assert.sameValue(Object.getOwnPropertyDescriptor(o,'a'),undefined);let order='';const ds={get 2(){order=order+'2';return {value:2};},get 1(){order=order+'1';return {value:1};},get a(){order=order+'a';return {value:3};}};Object.defineProperties(o,ds);assert.sameValue(order,'12a');");
}
#[test]
fn strict_this_call_and_arrow_lexical_capture() {
    check("function f(){return this;}assert.sameValue(f(),undefined);const o={f:f};assert.sameValue(o.f(),o);assert.sameValue(f.call(null),null);function maker(){return ()=>this;}const a=maker.call(o);assert.sameValue(a.call({}),o);assert.throws(TypeError,function(){new a();});");
}
#[test]
fn constructor_named_expression_and_instanceof() {
    check("const C=function Self(x){this.x=x;this.self=Self;};const o=new C(5);assert.sameValue(o.x,5);assert.sameValue(o.self,C);assert(o instanceof C);assert.sameValue(Object.getPrototypeOf(o),C.prototype);function D(){return {y:3};}assert.sameValue(new D().y,3);");
}
#[test]
fn finally_protects_pending_completions() {
    check("function f(){try{return {x:7};}finally{const scratch={a:{b:{}}};}}assert.sameValue(f().x,7);const marker={};function g(){try{throw marker;}finally{const scratch={};}}try{g();assert(false);}catch(e){assert.sameValue(e,marker);}function h(){try{return 1;}finally{return 2;}}assert.sameValue(h(),2);");
}
#[test]
fn accessor_exceptions_and_error_identity() {
    check("const marker={};const o={get x(){throw marker;},set x(v){throw new RangeError('bad');}};try{o.x;assert(false);}catch(e){assert.sameValue(e,marker);}assert.throws(RangeError,function(){o.x=1;});assert.sameValue(Object.getPrototypeOf(TypeError),Error);");
}
#[test]
fn key_coercion_before_descriptor_access() {
    check("let order='';const key={toString:function(){order=order+'k';return 'x';}};const desc={get value(){order=order+'v';return 9;}};const o={};Object.defineProperty(o,key,desc);assert.sameValue(order,'kv');assert.sameValue(o.x,9);");
}
#[test]
fn collector_follows_prototypes_getters_and_replacements() {
    check("function make(){const captured={n:8};return {get value(){return captured;}};}const p=make();const o=Object.create(p);o.x={old:1};o.x={new:2};assert.sameValue(o.value.n,8);assert.sameValue(o.x.new,2);");
}
#[test]
fn var_hoisting_tdz_and_mutability() {
    check("function f(){assert.sameValue(x,undefined);if(true){var x=3;}return x;}assert.sameValue(f(),3);assert.throws(ReferenceError,function(){x;let x=1;});assert.throws(TypeError,function(){const a=1;a=2;});let n=1;n=n+1;assert.sameValue(n,2);");
}
#[test]
fn sealed_and_frozen_objects() {
    check("const a={x:1};Object.seal(a);a.x=2;assert(Object.isSealed(a));assert.sameValue(Object.isFrozen(a),false);Object.freeze(a);assert(Object.isFrozen(a));assert.throws(TypeError,function(){a.x=3;});assert.throws(TypeError,function(){delete a.x;});assert.sameValue(a.x,2);assert(Object.isFrozen(1));");
}
#[test]
fn primitive_wrappers_and_string_indices() {
    check("const s=new String('ab');assert.sameValue(s.valueOf(),'ab');assert.sameValue(s.length,2);assert.sameValue(s[0],'a');assert.sameValue(Object.getOwnPropertyDescriptor(s,'0').configurable,false);assert.sameValue(new Number(5).valueOf(),5);assert.sameValue(new Boolean(false).valueOf(),false);assert.throws(TypeError,function(){String.prototype.valueOf.call({});});");
}
#[test]
fn strict_unmapped_arguments() {
    check("function f(x){assert.sameValue(arguments.length,1);assert.sameValue(arguments[0],4);x=5;assert.sameValue(arguments[0],4);arguments[0]=6;assert.sameValue(x,5);assert.throws(TypeError,function(){return arguments.callee;});return Object.prototype.toString.call(arguments);}assert.sameValue(f(4),'[object Arguments]');");
}
#[test]
fn bound_functions_call_and_construct() {
    check("function C(x,y){this.sum=x+y;return this;}const o={};const bound=C.bind(o,3);assert.sameValue(bound(4),o);assert.sameValue(o.sum,7);const n=new bound(5);assert.sameValue(n.sum,8);assert(n instanceof C);assert(n instanceof bound);assert.sameValue(bound.hasOwnProperty('prototype'),false);assert.sameValue(bound.length,1);assert.sameValue(bound.name,'bound C');");
}
#[test]
fn reflection_does_not_invoke_property_getters() {
    check("let count=0;const o={get prototype(){count=count+1;return {};}};Object.getOwnPropertyDescriptor(o,'prototype');assert.sameValue(count,0);const method={f(){return 1;}};assert.throws(TypeError,function(){new method.f();});assert.sameValue(method.f.hasOwnProperty('prototype'),false);const p=function(){};const a={__proto__:p};assert.sameValue(Object.getPrototypeOf(a),p);const b={__proto__(){}};assert.sameValue(Object.getPrototypeOf(b),Object.prototype);");
}
#[test]
fn numeric_property_key_formatting() {
    check("const o={1e21:1,1e-7:2};assert.sameValue(o['1e+21'],1);assert.sameValue(o['1e-7'],2);assert.sameValue(String(-0),'0');");
}
#[test]
fn assertion_ports_detect_false_positives() {
    for source in [
        "assert.sameValue(0,-0);",
        "assert.throws(TypeError,function(){throw new Error();});",
        "assert.throws(TypeError,function(){});",
        "assert(1);",
    ] {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.enable_test_harness();
        assert!(e.execute(source).is_err());
        assert_eq!(e.live_objects(), 0);
    }
}
#[test]
fn quota_failures_release_partial_intrinsics_and_graphs() {
    let mut passed = false;
    let mut failed = false;
    for budget in [0, 64, 512, 2048, 4096, 8192, 16384, 32768, 65536] {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.set_budget(Some(budget));
        let result =
            e.execute("const p={get x(){return {v:1};}};const o=Object.create(p);const x=o.x;");
        if result.is_ok() {
            passed = true;
        } else {
            failed = true;
            assert_eq!(e.live_objects(), 0);
        }
        e.release_globals();
        assert_eq!(
            (e.live_roots(), e.metadata_objects(), e.charged_bytes()),
            (0, 0, 0)
        );
    }
    assert!(passed && failed);
}
