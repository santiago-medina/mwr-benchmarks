//! Host exotic objects (HOST_EXOTIC.md, SERVO_BINDINGS.md E3) under the Stress
//! GC policy: essential internal methods answered by `Host::exotic`.
use gc_lab::{
    script::{
        Engine, ExoticOp, ExoticReply, Host, JsValue, ObjectId, PartialDescriptor,
        PropertyDescriptor, PropertyKey, ScriptError,
    },
    Model,
};
use std::{cell::RefCell, rc::Rc};

/// Every op answers `Default`: the object must behave as an ordinary object.
const ORDINARY: u64 = 1;
/// A WebIDL named-properties object: `named` is a configurable, non-enumerable
/// supported property name; defines of it and [[PreventExtensions]] are refused.
const NAMED: u64 = 2;
/// [[DefineOwnProperty]] re-enters and defines the same key on the same object
/// (or on its prototype), then answers `Default` (CVE-2022-1096 shape).
const REENTER_SELF: u64 = 3;
const REENTER_PROTO: u64 = 4;
/// [[Set]] makes the key a non-writable own data property, then answers `Default`.
const FREEZE_ON_SET: u64 = 5;
/// Answers every op with a reply of the wrong shape.
const BROKEN: u64 = 6;

#[derive(Default)]
struct ExoticHost {
    log: RefCell<Vec<String>>,
    inside: RefCell<bool>,
}

fn key_str(k: &PropertyKey) -> String {
    match k {
        PropertyKey::String(s) => s.clone(),
        PropertyKey::Symbol(s) => format!("@{s}"),
    }
}

impl Host for ExoticHost {
    fn call(
        &self,
        _engine: &mut Engine<'_>,
        _function: u64,
        _this: JsValue,
        _args: &[JsValue],
        _new_target: Option<ObjectId>,
    ) -> Result<JsValue, ScriptError> {
        unreachable!()
    }

    fn exotic(
        &self,
        engine: &mut Engine<'_>,
        object: ObjectId,
        handler: u64,
        op: ExoticOp,
    ) -> Result<ExoticReply, ScriptError> {
        self.log.borrow_mut().push(format!("{op:?}"));
        if *self.inside.borrow() {
            return Ok(ExoticReply::Default);
        }
        match (handler, op) {
            (NAMED, ExoticOp::GetOwnProperty(PropertyKey::String(k))) if k == "named" => {
                Ok(ExoticReply::Property(Some(PropertyDescriptor {
                    value: Some(JsValue::number(42.0)),
                    writable: true,
                    getter: None,
                    setter: None,
                    enumerable: false,
                    configurable: true,
                })))
            }
            (NAMED, ExoticOp::DefineOwnProperty(PropertyKey::String(k), _)) if k == "named" => {
                Ok(ExoticReply::Bool(false))
            }
            (NAMED, ExoticOp::PreventExtensions) => Ok(ExoticReply::Bool(false)),
            (NAMED, ExoticOp::OwnPropertyKeys) => {
                // Supported names first, then the ordinary own keys.
                *self.inside.borrow_mut() = true;
                let rest = engine.own_property_keys(object);
                *self.inside.borrow_mut() = false;
                let mut keys = vec![PropertyKey::from("named")];
                keys.extend(rest?);
                Ok(ExoticReply::Keys(keys))
            }
            (REENTER_SELF | REENTER_PROTO, ExoticOp::DefineOwnProperty(k, _)) => {
                let target = if handler == REENTER_SELF {
                    object
                } else {
                    engine.prototype_of(object)?.unwrap()
                };
                *self.inside.borrow_mut() = true;
                let r = engine.define_own_property(
                    target,
                    k,
                    PartialDescriptor {
                        value: Some(JsValue::string("from trap")),
                        writable: Some(true),
                        enumerable: Some(true),
                        configurable: Some(true),
                        ..PartialDescriptor::default()
                    },
                );
                *self.inside.borrow_mut() = false;
                r?;
                Ok(ExoticReply::Default)
            }
            (FREEZE_ON_SET, ExoticOp::Set(k, ..)) => {
                *self.inside.borrow_mut() = true;
                let r = engine.define_property(object, k, JsValue::number(1.0), false, true, true);
                *self.inside.borrow_mut() = false;
                r?;
                Ok(ExoticReply::Default)
            }
            (BROKEN, ExoticOp::Get(..) | ExoticOp::OwnPropertyKeys) => Ok(ExoticReply::Bool(true)),
            (BROKEN, _) => Ok(ExoticReply::Keys(vec![])),
            _ => Ok(ExoticReply::Default),
        }
    }
}

fn setup(handler: u64) -> (Engine<'static>, Rc<ExoticHost>, ObjectId) {
    let host = Rc::new(ExoticHost::default());
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.set_step_limit(1_000_000);
    e.set_host(host.clone());
    let global = e.init_realm().unwrap();
    let proto = e.create_object(e.object_prototype()).unwrap();
    let root = e.root(proto).unwrap();
    let o = e
        .create_host_exotic(Some(proto), handler, 0x5eed, 7, 1, false)
        .unwrap();
    let root_o = e.root(o).unwrap();
    assert!(e
        .define_property(global, "o", JsValue::object(o), true, false, true)
        .unwrap());
    assert!(e
        .define_property(global, "proto", JsValue::object(proto), true, false, true)
        .unwrap());
    e.unroot(root_o).unwrap();
    e.unroot(root).unwrap();
    (e, host, o)
}

fn eval_str(e: &mut Engine<'_>, src: &str) -> String {
    match e.evaluate_script(src) {
        Ok(v) => e.to_string_value(&v).unwrap(),
        Err(err) => {
            let thrown = e.exception_value(&err).expect("thrown value");
            format!("threw {}", e.to_string_value(&thrown).unwrap())
        }
    }
}

#[test]
fn default_replies_behave_as_an_ordinary_object() {
    let (mut e, host, o) = setup(ORDINARY);
    assert_eq!(e.exotic_handler_of(o), Some(ORDINARY));
    assert_eq!(e.host_class(o), Some((0x5eed, 7)));
    e.set_reserved_slot(o, 0, JsValue::string("slot")).unwrap();
    let r = eval_str(
        &mut e,
        r#"
        o.a = 1; proto.inherited = 2;
        Object.defineProperty(o, 'b', { value: 3, enumerable: false, configurable: true });
        var out = [typeof o, Array.isArray(o), o.a, o.inherited, 'a' in o, 'inherited' in o,
          'zz' in o, Object.keys(o).join(), Reflect.ownKeys(o).join(),
          Object.getPrototypeOf(o) === proto, delete o.a, 'a' in o, Object.isExtensible(o)];
        for (var k in o) out.push('for-in:' + k);
        Object.preventExtensions(o);
        out.push(Object.isExtensible(o), Reflect.set(o, 'c', 1), o.c);
        out.join('|')
        "#,
    );
    assert_eq!(
        r,
        "object|false|1|2|true|true|false|a|a,b|true|true|false|true|for-in:inherited|false|false|"
    );
    assert_eq!(
        e.reserved_slot(o, 0).unwrap().as_string().as_deref(),
        Some("slot")
    );
    let log = host.log.borrow();
    assert!(log.iter().any(|l| l.starts_with("Get(")));
    assert!(log.iter().any(|l| l.starts_with("Set(")));
    assert!(log.iter().any(|l| l == "OwnPropertyKeys"));
    assert!(log.iter().any(|l| l == "PreventExtensions"));
}

#[test]
fn named_properties_object_traps() {
    let (mut e, _host, o) = setup(NAMED);
    let r = eval_str(
        &mut e,
        r#"
        o.expando = 5;
        var d = Object.getOwnPropertyDescriptor(o, 'named');
        [o.named, 'named' in o, o.hasOwnProperty('named'), d.value, d.enumerable,
         d.configurable, Object.keys(o).join(), Object.getOwnPropertyNames(o).join(),
         Reflect.defineProperty(o, 'named', { value: 1 }), Reflect.preventExtensions(o),
         Object.isExtensible(o), o.missing].join('|')
        "#,
    );
    assert_eq!(
        r,
        "42|true|true|42|false|true|expando|named,expando|false|false|true|"
    );
    let r = eval_str(
        &mut e,
        "try { Object.preventExtensions(o); 'no throw' } catch (x) { x.name }",
    );
    assert_eq!(r, "TypeError");
    // JS_SetImmutablePrototype on the named-properties object (window_named_properties).
    assert!(e.set_immutable_prototype(o).unwrap());
    let r = eval_str(
        &mut e,
        r#"[Reflect.setPrototypeOf(o, {}), Reflect.setPrototypeOf(o, proto),
            (function () { try { Object.setPrototypeOf(o, null) } catch (x) { return x.name } })()
           ].join('|')"#,
    );
    assert_eq!(r, "false|true|TypeError");
    e.collect();
    assert!(e.is_alive(o));
}

/// HOST_EXOTIC.md "Re-entrancy after Default": the trap defines the same key,
/// then the engine's ordinary [[DefineOwnProperty]] runs on the fresh state.
#[test]
fn reentrant_define_then_default_leaves_one_property() {
    let (mut e, host, _o) = setup(REENTER_SELF);
    let r = eval_str(
        &mut e,
        r#"
        Object.defineProperty(o, 'k', { value: 'from script', writable: true,
          enumerable: true, configurable: true });
        [o.k, Reflect.ownKeys(o).join(), Object.keys(o).length].join('|')
        "#,
    );
    assert_eq!(r, "from script|k|1");
    // One op for the outer [[DefineOwnProperty]], one for the trap's own call.
    let defines = host
        .log
        .borrow()
        .iter()
        .filter(|l| l.starts_with("DefineOwnProperty"))
        .count();
    assert_eq!(defines, 2);
}

/// The same with the trap writing to the prototype (CVE-2021-30551 path).
#[test]
fn reentrant_define_on_prototype_then_default() {
    let (mut e, _host, _o) = setup(REENTER_PROTO);
    let r = eval_str(
        &mut e,
        r#"
        o.k = 'from script';
        [o.k, proto.k, Reflect.ownKeys(o).join(), Reflect.ownKeys(proto).join()].join('|')
        "#,
    );
    assert_eq!(r, "from script|from trap|k|k");
}

/// The trap makes the key read-only; the Default [[Set]] must see that.
#[test]
fn reentrant_set_rereads_the_own_descriptor() {
    let (mut e, _host, _o) = setup(FREEZE_ON_SET);
    let r = eval_str(
        &mut e,
        r#"
        var sloppy = Reflect.set(o, 'k', 2);
        var strict = (function () { 'use strict'; try { o.k = 3; return 'no throw' }
          catch (x) { return x.name } })();
        [sloppy, strict, o.k, Reflect.ownKeys(o).join()].join('|')
        "#,
    );
    assert_eq!(r, "false|TypeError|1|k");
}

/// HOST_EXOTIC.md "Receiver" (CVE-2013-1737): accessors found on the
/// prototype see the host exotic as `this`, not the holder.
#[test]
fn default_get_and_set_forward_the_receiver() {
    let (mut e, _host, _o) = setup(ORDINARY);
    let r = eval_str(
        &mut e,
        r#"
        var seen = [];
        Object.defineProperty(proto, 'g', { get() { return this }, configurable: true });
        Object.defineProperty(proto, 's', { set(v) { seen.push(this === o, v) },
          configurable: true });
        o.s = 7;
        var other = {};
        [o.g === o, seen.join(), Reflect.get(o, 'g', other) === other].join('|')
        "#,
    );
    assert_eq!(r, "true|true,7|true");
}

#[test]
fn replies_of_the_wrong_shape_are_type_errors() {
    let (mut e, _host, _o) = setup(BROKEN);
    let r = eval_str(
        &mut e,
        r#"
        var out = [];
        for (var f of [() => o.x, () => 'x' in o, () => Object.keys(o),
                       () => Object.getPrototypeOf(o)]) {
          try { out.push(String(f())) } catch (x) { out.push(x.name) }
        }
        out.join('|')
        "#,
    );
    assert_eq!(r, "TypeError|TypeError|TypeError|TypeError");
}
