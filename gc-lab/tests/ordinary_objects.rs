use gc_lab::{script::Engine, Model};
fn check(source: &str) {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.enable_test_harness();
        e.execute(&format!("'use strict';{source}"))
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
fn computed_assignment_defers_property_key_conversion() {
    check("let order='';const target={};const key={toString(){order+='k';return 'x';}};function rhs(){order+='r';return 7;}target[key]=rhs();assert.sameValue(order,'rk');assert.sameValue(target.x,7);");
    check("let order='';const target={x:1};const key={toString(){order+='k';return 'x';}};function rhs(){order+='r';return 2;}target[key]+=rhs();if(order!=='kr')throw Error('order:'+order);assert.sameValue(target.x,3);");
}
#[test]
fn super_base_precedes_computed_key_coercion() {
    check("const proto={p:'old'};const next={p:'new'};const obj={__proto__:proto,m(){return super[key];}};const key={toString(){Object.setPrototypeOf(obj,next);return 'p';}};assert.sameValue(obj.m(),'old');assert.sameValue(Object.getPrototypeOf(obj),next);");
}
#[test]
fn set_algebra_normalizes_negative_zero() {
    check("const minus={size:1,has(){return true;},keys(){return [-0].values();}};const u=new Set([1]).union(minus);assert.sameValue(Object.is([...u][1],+0),true);const s=new Set([1]).symmetricDifference(minus);assert.sameValue(Object.is([...s][1],+0),true);const i=new Set([0,1,2]).intersection(minus);assert.sameValue(Object.is([...i][0],+0),true);");
}
#[test]
fn regexp_symbol_hooks_are_not_read_on_primitive_arguments() {
    check("Object.defineProperty(Number.prototype,Symbol.match,{get(){throw Error('match hook');}});assert.sameValue('a1b'.match(1)[0],'1');Object.defineProperty(Boolean.prototype,Symbol.search,{get(){throw Error('search hook');}});assert.sameValue('atrueb'.search(true),1);Object.defineProperty(String.prototype,Symbol.matchAll,{get(){throw Error('matchAll hook');}});const matches=Array.from('a,b,c'.matchAll(','));assert.sameValue(matches.length,2);assert.sameValue(matches[0].index,1);");
}
#[test]
fn string_symbol_function_call_vs_constructor() {
    check("const symbol=Symbol('66');assert.sameValue(String(symbol),'Symbol(66)');assert.throws(TypeError,function(){new String(symbol);});assert.throws(TypeError,function(){Reflect.construct(String,[symbol]);});");
}
#[test]
fn object_tostring_uses_inherited_tag_and_object_fallback_for_special_primitives() {
    check("const iterator=[][Symbol.iterator]();const p=Object.getPrototypeOf(iterator);assert.sameValue(Object.prototype.toString.call(iterator),'[object Array Iterator]');Object.defineProperty(p,Symbol.toStringTag,{configurable:true,value:null});assert.sameValue(Object.prototype.toString.call(iterator),'[object Object]');delete p[Symbol.toStringTag];assert.sameValue(Object.prototype.toString.call(iterator),'[object Iterator]');");
    check("delete Symbol.prototype[Symbol.toStringTag];assert.sameValue(Object.prototype.toString.call(Symbol('x')),'[object Object]');Object.defineProperty(BigInt.prototype,Symbol.toStringTag,{value:null});assert.sameValue(Object.prototype.toString.call(BigInt(0)),'[object Object]');assert.sameValue(Object.prototype.toString.call(Object(BigInt(0))),'[object Object]');");
}
#[test]
fn symbol_and_bigint_are_constructible_but_reject_construction() {
    check("assert.sameValue(typeof Symbol,'function');assert.sameValue(typeof BigInt,'function');let n=0;assert.throws(TypeError,function(){new BigInt({valueOf(){n++;return 1;}});});assert.sameValue(n,0);assert.throws(TypeError,function(){new Symbol('s');});assert.throws(TypeError,function(){Reflect.construct(BigInt,[]);});assert.throws(TypeError,function(){Reflect.construct(Symbol,[]);});");
}
#[test]
fn numeric_left_conversion_precedes_right_conversion() {
    check("let trace='';const left={valueOf(){trace+='L';return Symbol('n');}};const right={valueOf(){trace+='R';throw Error('right must not convert');}};for(const op of ['-','*','/','%','**','&','|','^','<<','>>','>>>']){trace='';assert.throws(TypeError,function(){Function('a','b','return a '+op+' b')(left,right);});assert.sameValue(trace,'L');}");
}
#[test]
fn bigint_prototype_is_not_a_wrapper_and_tostring_has_zero_length() {
    check("assert.sameValue(BigInt.prototype.toString.length,0);assert.sameValue(BigInt.prototype.valueOf.length,0);assert.sameValue(BigInt.asIntN.length,2);assert.sameValue(BigInt.asUintN.length,2);assert.throws(TypeError,function(){BigInt.prototype.toString();});assert.throws(TypeError,function(){BigInt.prototype.valueOf();});assert.sameValue(123n.toString(),'123');");
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
    for budget in [
        0,
        64,
        512,
        4096,
        32768,
        65536,
        1 << 18,
        1 << 20,
        1 << 22,
        1 << 24,
    ] {
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
#[test]
fn global_var_property_record_and_constant_descriptors() {
    check("Object.defineProperty(Object.prototype,'prop',{value:1001});var prop=1002;assert(this.hasOwnProperty('prop'));assert.sameValue(this.prop,1002);this.prop=1003;assert.sameValue(prop,1003);prop=1004;assert.sameValue(this.prop,1004);assert.sameValue(Object.getOwnPropertyDescriptor(this,'prop').configurable,false);assert.sameValue(Object.getOwnPropertyDescriptor(this,'NaN').writable,false);assert.sameValue(Object.getOwnPropertyDescriptor(this,'undefined').configurable,false);");
}
#[test]
fn failed_public_lookup_does_not_escape_exception_roots() {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.execute("const keep={};").unwrap();
    for _ in 0..4 {
        assert!(e.call_global("missing").is_err());
        assert!(e.binding_string("also_missing").is_err());
    }
    e.release_globals();
    assert_eq!(
        (e.live_objects(), e.metadata_objects(), e.live_roots()),
        (0, 0, 0)
    );
}
#[test]
fn allocation_failure_inside_existing_realm_preserves_roots_and_recovery() {
    let mut successes = 0;
    let mut failures = 0;
    for model in [Model::Stable, Model::Indirect] {
        for extra in [0, 16, 64, 128, 256, 512, 1024, 2048, 4096] {
            let mut e = Engine::new(model, model).unwrap();
            e.execute("const root={keep:{value:1}};function work(){root.other={a:{b:2}};Object.defineProperty(root,'accessor',{get:function(){return root.keep;},configurable:true});}function inspect(){if(root.keep.value!==1){throw 'corrupt';}}").unwrap();
            let base = e.charged_bytes();
            e.set_budget(Some(base + extra));
            if e.call_global("work").is_ok() {
                successes += 1;
            } else {
                failures += 1;
            }
            e.collect();
            assert_eq!(e.live_roots(), 1);
            assert_eq!(e.live_objects(), e.metadata_objects());
            e.set_budget(None);
            e.call_global("inspect").unwrap();
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
    assert!(successes > 0 && failures > 0);
}
