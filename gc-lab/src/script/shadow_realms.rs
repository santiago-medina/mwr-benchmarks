//! ShadowRealm (TC39 proposal-shadowrealm, Stage 2.7; spec text at
//! https://tc39.es/proposal-shadowrealm/). Built on the realm records of realms.rs
//! (REALMS.md): a ShadowRealm object holds a realm index in [[ShadowRealm]], and values
//! cross the boundary only as primitives or wrapped function exotic objects.
use std::collections::HashMap;

use super::{
    objects::{Data, Descriptor, Key},
    parser,
    promises::Internal,
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;

impl Engine<'_> {
    /// %ShadowRealm.prototype% with `evaluate`, `importValue` and @@toStringTag.
    pub(super) fn bootstrap_shadow_realm(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let object = self.intrinsics.as_ref().unwrap().object;
        let function = self.intrinsics.as_ref().unwrap().function;
        let proto = self.new_object(Some(object))?;
        self.runtime.link(env, proto)?;
        objects.insert("ShadowRealm.prototype".into(), proto);
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("ShadowRealm".into(), proto);
        self.define(
            objects["ShadowRealm"],
            "length",
            Descriptor {
                value: Some(Value::number(0.0)),
                ..Descriptor::default()
            },
        )?;
        // ShadowRealm.prototype [ %Symbol.toStringTag% ]: configurable only.
        self.define(
            proto,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("ShadowRealm".into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        for (method, length) in [("evaluate", 1), ("importValue", 2)] {
            let f = self.intrinsic_function(
                env,
                &format!("ShadowRealm.prototype.{method}"),
                Some(function),
            )?;
            self.define(
                f,
                "length",
                Descriptor {
                    value: Some(Value::number(length as f64)),
                    ..Descriptor::default()
                },
            )?;
            self.hidden(proto, method, Value::Object(f))?;
        }
        Ok(())
    }

    /// ShadowRealm ( ) [[Construct]] (§3.2.1): OrdinaryCreateFromConstructor first, then a
    /// new realm (InitializeHostDefinedRealm; HostInitializeShadowRealm adds nothing).
    pub(super) fn construct_shadow_realm(&mut self, new_target: Value) -> Result<Value> {
        let proto = self.prototype_from_constructor(new_target, |i| i.collection["ShadowRealm"])?;
        let id = self.new_object(Some(proto))?;
        let result = Value::Object(id);
        self.protect(&result)?;
        let realm = self.new_realm()?;
        let mut data = self.data(id)?.clone();
        data.shadow_realm = Some(realm);
        self.replace_data(id, data)?;
        Ok(result)
    }

    pub(super) fn shadow_realm_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if name == "ShadowRealm" {
            return self.throw_error("TypeError", "Constructor ShadowRealm requires 'new'");
        }
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        // ValidateShadowRealmObject (§3.1.3).
        let realm = match &receiver {
            Value::Object(id) if !self.is_proxy(*id) => {
                self.data(*id).ok().and_then(|d| d.shadow_realm)
            }
            _ => None,
        };
        let Some(realm) = realm else {
            return self.throw_error(
                "TypeError",
                &format!("{name} called on incompatible receiver"),
            );
        };
        match name {
            // §3.4.1 ShadowRealm.prototype.evaluate ( sourceText ).
            "ShadowRealm.prototype.evaluate" => {
                let Value::String(text) = arg(0) else {
                    return self.throw_error("TypeError", "evaluate expects a string");
                };
                self.perform_shadow_realm_eval(Value::String(text), realm)
            }
            // §3.4.2 importValue needs HostLoadImportedModule from a realm without a
            // referrer script; not implemented yet.
            _ => Err(ScriptError::Unsupported(
                "ShadowRealm.prototype.importValue",
            )),
        }
    }

    /// PerformShadowRealmEval (§3.1.1) of `source` in realm `eval_realm`; the running realm
    /// is the caller realm.
    fn perform_shadow_realm_eval(&mut self, source: Value, eval_realm: usize) -> Result<Value> {
        let Value::String(text) = &source else {
            unreachable!()
        };
        // Steps 4-5: ParseText as a Script in the caller realm; early errors (including
        // new.target, super and `arguments`-free checks of inFunction = false) are a
        // SyntaxError from the caller realm.
        match parser::parse(text.text()?) {
            Ok(_) => {}
            Err(error)
                if error.message.contains("unsupported") || error.message.ends_with("limit") =>
            {
                return Err(ScriptError::Syntax(error))
            }
            Err(error) => return self.throw_error("SyntaxError", error.message),
        }
        // Steps 6-21: evaluation in evalRealm with varEnv = its global environment and a
        // fresh lexical environment, as an indirect eval there (PerformEval with no caller).
        let result = self.in_realm(eval_realm, |e| e.perform_eval(source, None));
        match result {
            Ok(value) => self.get_wrapped_value(value),
            // Step 22: an abrupt completion becomes a TypeError of the caller realm.
            Err(ScriptError::Thrown(_)) => {
                self.throw_error("TypeError", "ShadowRealm evaluation threw")
            }
            Err(other) => Err(other),
        }
    }

    /// GetWrappedValue (§2.3) into the running realm.
    pub(super) fn get_wrapped_value(&mut self, value: Value) -> Result<Value> {
        if !matches!(value, Value::Object(_)) {
            return Ok(value);
        }
        if !self.callable(&value) {
            return self.throw_error(
                "TypeError",
                "cross-realm value is not a primitive or callable",
            );
        }
        self.wrapped_function_create(value)
    }

    /// WrappedFunctionCreate (§2.1) in the running realm (the caller realm).
    fn wrapped_function_create(&mut self, target: Value) -> Result<Value> {
        let function = self.intrinsics.as_ref().unwrap().function;
        let id = self.new_object(Some(function))?;
        let wrapped = Value::Object(id);
        self.protect(&wrapped)?;
        if let Some(child) = target.id() {
            self.runtime.link(id, child)?;
        }
        let mut data = self.data(id)?.clone();
        data.internal = Some(Internal::Wrapped {
            target: target.clone(),
        });
        self.objects.insert(
            id,
            Object::Builtin {
                name: "%WrappedFunction%".into(),
                data: Data::new(None),
            },
        );
        self.replace_data(id, data)?;
        // Step 7: CopyNameAndLength; an abrupt completion becomes a TypeError.
        match self.copy_name_and_length(id, target) {
            Ok(()) => Ok(wrapped),
            Err(ScriptError::Thrown(_)) => {
                self.throw_error("TypeError", "wrapped function name or length threw")
            }
            Err(other) => Err(other),
        }
    }

    /// CopyNameAndLength (§2.4) with no prefix and argCount 0.
    fn copy_name_and_length(&mut self, f: Id, target: Value) -> Result<()> {
        let Value::Object(target_id) = target else {
            unreachable!()
        };
        let mut length = 0.0;
        if self.own(target_id, "length")?.is_some() {
            let target_len = self.get_member(target.clone(), "length")?;
            if let Value::Number(bits) = target_len {
                let n = f64::from_bits(bits);
                length = if n == f64::INFINITY {
                    f64::INFINITY
                } else if n == f64::NEG_INFINITY {
                    0.0
                } else {
                    // ToIntegerOrInfinity, then max(L - argCount, 0).
                    let n = if n.is_nan() { 0.0 } else { n.trunc() };
                    n.max(0.0)
                };
            }
        }
        // SetFunctionLength: { [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true }.
        self.define(
            f,
            "length",
            Descriptor {
                value: Some(Value::number(length)),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        let name = match self.get_member(target, "name")? {
            Value::String(s) => Value::String(s),
            _ => Value::String("".into()),
        };
        self.define(
            f,
            "name",
            Descriptor {
                value: Some(name),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(())
    }

    /// [[Call]] of a wrapped function exotic object (§2.1.1). The running realm is already
    /// F.[[Realm]] (the caller realm: `invoke` entered the realm the wrapper was made in).
    pub(super) fn wrapped_call(
        &mut self,
        target: Value,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let result = (|| {
            let target_realm = self.function_realm(&target)?;
            let (receiver, args) = self.in_realm(target_realm, |e| {
                let mut wrapped = Vec::with_capacity(args.len());
                for arg in args {
                    let value = e.get_wrapped_value(arg)?;
                    e.protect(&value)?;
                    wrapped.push(value);
                }
                let receiver = e.get_wrapped_value(receiver)?;
                e.protect(&receiver)?;
                Ok((receiver, wrapped))
            })?;
            self.invoke_this(target.clone(), receiver, args)
        })();
        match result {
            Ok(value) => self.get_wrapped_value(value),
            // Step 10: any abrupt completion of the call becomes a caller-realm TypeError.
            Err(ScriptError::Thrown(_)) => self.throw_error("TypeError", "wrapped function threw"),
            Err(other) => Err(other),
        }
    }
}
