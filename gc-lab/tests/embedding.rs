//! The embedding API (SERVO_BINDINGS.md E1, E2, E4, E5) under the Stress GC policy,
//! which collects at every allocation and so catches unrooted ids.
use gc_lab::{
    script::{Engine, Host, JsValue, ObjectId, PropertyKey, ScriptError},
    Model,
};
use std::{cell::RefCell, rc::Rc};

const ADD: u64 = 1;
const POINT: u64 = 2;
const FAIL: u64 = 3;
const CALL_BACK: u64 = 4;
const POINT_CLASS: u64 = 77;

#[derive(Default)]
struct TestHost {
    point_proto: RefCell<Option<ObjectId>>,
    /// Edges a traced host object holds outside its slots, keyed by payload.
    hidden: RefCell<Vec<(u64, ObjectId)>>,
    extra_roots: RefCell<Vec<ObjectId>>,
    finalized: RefCell<Vec<(u64, u64)>>,
}

impl Host for TestHost {
    fn call(
        &self,
        engine: &mut Engine<'_>,
        function: u64,
        this: JsValue,
        args: &[JsValue],
        new_target: Option<ObjectId>,
    ) -> Result<JsValue, ScriptError> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(JsValue::undefined());
        match function {
            ADD => {
                let a = arg(0).as_number().unwrap_or(f64::NAN);
                let b = arg(1).as_number().unwrap_or(f64::NAN);
                Ok(JsValue::number(a + b))
            }
            POINT => {
                let Some(target) = new_target else {
                    return Err(engine.throw_new_error("TypeError", "Point requires new"));
                };
                // GetPrototypeFromConstructor (§10.1.14).
                let proto = engine.get_property(target, "prototype")?.as_object();
                let point = engine.create_host_object(proto, POINT_CLASS, 5, 2, false)?;
                let root = engine.root(point)?;
                engine.set_reserved_slot(point, 0, arg(0))?;
                engine.set_reserved_slot(point, 1, arg(1))?;
                engine.unroot(root)?;
                Ok(JsValue::object(point))
            }
            FAIL => Err(engine.throw_new_error("RangeError", "host says no")),
            CALL_BACK => {
                let result = engine.call_function(&arg(0), &this, &[JsValue::number(41.0)]);
                match result {
                    Ok(v) => Ok(JsValue::number(v.as_number().unwrap() + 1.0)),
                    Err(error) => {
                        let thrown = engine.exception_value(&error).expect("thrown value");
                        let message =
                            engine.get_property(thrown.as_object().unwrap(), "message")?;
                        Ok(JsValue::string(&format!(
                            "caught {}",
                            message.as_string().unwrap()
                        )))
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    fn trace_roots(&self, out: &mut Vec<ObjectId>) {
        out.extend(self.extra_roots.borrow().iter().copied());
        out.extend(*self.point_proto.borrow());
    }
    fn trace(&self, _object: ObjectId, _class: u64, payload: u64, out: &mut Vec<ObjectId>) {
        out.extend(
            self.hidden
                .borrow()
                .iter()
                .filter(|(p, _)| *p == payload)
                .map(|(_, o)| *o),
        );
    }
    fn finalize(&self, class: u64, payload: u64) {
        self.finalized.borrow_mut().push((class, payload));
    }
}

fn engine(host: &Rc<TestHost>) -> (Engine<'static>, ObjectId) {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.set_step_limit(1_000_000);
    e.set_host(host.clone());
    let global = e.init_realm().unwrap();
    (e, global)
}

fn define_function(
    e: &mut Engine<'_>,
    on: ObjectId,
    name: &str,
    word: u64,
    ctor: bool,
) -> ObjectId {
    let f = e.create_function(name, 2, word, ctor).unwrap();
    assert!(e
        .define_property(on, name, JsValue::object(f), true, false, true)
        .unwrap());
    f
}

#[test]
fn host_functions_are_called_and_constructed_from_script() {
    let host = Rc::new(TestHost::default());
    let (mut e, global) = engine(&host);
    define_function(&mut e, global, "add", ADD, false);
    let point = define_function(&mut e, global, "Point", POINT, true);
    let proto = e.create_object(e.object_prototype()).unwrap();
    *host.point_proto.borrow_mut() = Some(proto);
    e.define_property(
        point,
        "prototype",
        JsValue::object(proto),
        false,
        false,
        false,
    )
    .unwrap();
    e.define_property(proto, "kind", JsValue::string("point"), false, false, true)
        .unwrap();

    let v = e.evaluate_script("add(2, 40)").unwrap();
    assert_eq!(v.as_number(), Some(42.0));
    let v = e
        .evaluate_script("typeof add + ':' + add.name + ':' + add.length")
        .unwrap();
    assert_eq!(v.as_string().as_deref(), Some("function:add:2"));
    let v = e
        .evaluate_script("var p = new Point(3, 4); p.kind")
        .unwrap();
    assert_eq!(v.as_string().as_deref(), Some("point"));
    // The global persists between scripts; the slots survived the Stress collections.
    let p = e.evaluate_script("p").unwrap().as_object().unwrap();
    assert_eq!(e.host_class(p), Some((POINT_CLASS, 5)));
    assert_eq!(e.reserved_slot(p, 1).unwrap().as_number(), Some(4.0));
    // Subclassing passes new.target through to the host constructor.
    let v = e
        .evaluate_script("class P3 extends Point { z() { return 9 } }; new P3(1, 2).z()")
        .unwrap();
    assert_eq!(v.as_number(), Some(9.0));
    // [[Call]] of a constructor without new reaches the host with no new_target.
    let error = e.evaluate_script("Point(1, 2)").unwrap_err();
    let thrown = e.exception_value(&error).unwrap();
    assert_eq!(
        e.to_string_value(&thrown).unwrap(),
        "TypeError: Point requires new"
    );
    // add is not a constructor.
    let v = e
        .evaluate_script("try { new add(); 'no' } catch (err) { err.constructor.name }")
        .unwrap();
    assert_eq!(v.as_string().as_deref(), Some("TypeError"));
}

#[test]
fn errors_cross_the_boundary_both_ways() {
    let host = Rc::new(TestHost::default());
    let (mut e, global) = engine(&host);
    define_function(&mut e, global, "fail", FAIL, false);
    define_function(&mut e, global, "callBack", CALL_BACK, false);
    let v = e
        .evaluate_script("try { fail() } catch (err) { err instanceof RangeError && err.message }")
        .unwrap();
    assert_eq!(v.as_string().as_deref(), Some("host says no"));
    let v = e.evaluate_script("callBack(x => x)").unwrap();
    assert_eq!(v.as_number(), Some(42.0));
    let v = e
        .evaluate_script("callBack(() => { throw new Error('from script') })")
        .unwrap();
    assert_eq!(v.as_string().as_deref(), Some("caught from script"));
    // A syntax error is a thrown SyntaxError, and the realm keeps working.
    let error = e.evaluate_script("let = = 1").unwrap_err();
    let thrown = e.exception_value(&error).unwrap();
    let name = e.get_property(thrown.as_object().unwrap(), "name").unwrap();
    assert_eq!(name.as_string().as_deref(), Some("SyntaxError"));
    assert_eq!(e.evaluate_script("1 + 1").unwrap().as_number(), Some(2.0));
}

#[test]
fn host_objects_are_traced_rooted_and_finalized() {
    let host = Rc::new(TestHost::default());
    let (mut e, global) = engine(&host);
    // A traced host object reachable from the global keeps its hidden edge alive.
    let owner = e.create_host_object(None, 9, 100, 1, true).unwrap();
    e.define_property(global, "owner", JsValue::object(owner), true, false, true)
        .unwrap();
    let hidden = e.create_object(None).unwrap();
    host.hidden.borrow_mut().push((100, hidden));
    // Slot edge.
    let slotted = e.create_object(None).unwrap();
    e.set_reserved_slot(owner, 0, JsValue::object(slotted))
        .unwrap();
    // An extra root keeps an otherwise unreachable object alive.
    let extra = e.create_host_object(None, 9, 200, 0, false).unwrap();
    host.extra_roots.borrow_mut().push(extra);
    // Garbage host object.
    let garbage = e.create_host_object(None, 9, 300, 0, false).unwrap();
    e.collect();
    for id in [owner, hidden, slotted, extra] {
        assert!(e.is_alive(id));
    }
    assert!(!e.is_alive(garbage));
    assert_eq!(*host.finalized.borrow(), vec![(9, 300)]);

    // Unreachable once the global drops it: the owner is finalized, its edges die.
    e.delete_property(global, "owner").unwrap();
    host.extra_roots.borrow_mut().clear();
    e.collect();
    for id in [owner, hidden, slotted, extra] {
        assert!(!e.is_alive(id));
    }
    let mut finalized = host.finalized.borrow().clone();
    finalized.sort();
    assert_eq!(finalized, vec![(9, 100), (9, 200), (9, 300)]);
    // A stale id reported by the host is ignored, not an error.
    host.extra_roots.borrow_mut().push(extra);
    e.collect();
}

#[test]
fn properties_and_keys() {
    let host = Rc::new(TestHost::default());
    let (mut e, _global) = engine(&host);
    let o = e.create_object(e.object_prototype()).unwrap();
    let root = e.root(o).unwrap();
    e.set_property(o, "a", JsValue::number(1.0)).unwrap();
    e.define_property(o, "b", JsValue::boolean(true), false, true, false)
        .unwrap();
    let getter = e.create_function("get c", 0, ADD, false).unwrap();
    e.define_accessor(o, "c", Some(getter), None, true, true)
        .unwrap();
    assert!(e
        .get_property(o, "c")
        .unwrap()
        .as_number()
        .unwrap()
        .is_nan());
    assert_eq!(
        e.own_property_keys(o).unwrap(),
        vec![
            PropertyKey::String("a".into()),
            PropertyKey::String("b".into()),
            PropertyKey::String("c".into())
        ]
    );
    assert!(!e.delete_property(o, "b").unwrap());
    assert!(e.delete_property(o, "a").unwrap());
    assert_eq!(e.prototype_of(o).unwrap(), e.object_prototype());
    e.unroot(root).unwrap();
}
