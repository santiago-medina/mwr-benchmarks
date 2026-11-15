//! A reference is evaluated once, before the value that will be assigned to it.
use super::{
    objects::Key,
    parser::Expr,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
use std::cell::RefCell;
type Result<T> = std::result::Result<T, ScriptError>;
pub(super) enum Reference<'a> {
    /// Captured resolved Environment Record, identifier name and strictness.
    Binding(Id, &'a str, bool),
    /// A name resolved to a `with` object environment (§9.1.1.2): the binding
    /// object, the name and strictness. Resolved once, so traps run once.
    WithBinding(Id, &'a str, bool),
    /// A strict-mode name that resolved nowhere before the right-hand side ran.
    Unresolvable(&'a str),
    /// Base, key, and whether the reference is in strict mode code.
    Property(Value, Key, bool),
    /// Computed reference retains the key value until GetValue/PutValue.
    /// For plain assignment ToPropertyKey runs after RHS evaluation (§6.2.5).
    Computed(Box<(Value, Value, RefCell<Option<Key>>, bool)>),
    /// `this`, the [[HomeObject]]'s prototype, the key and strictness.
    Super(Value, Option<Id>, Key, bool),
    /// `super[expr]`: `this`, the home prototype, the unconverted key value (with its
    /// key once GetValue converted it) and strictness.
    SuperComputed(Box<(Value, Option<Id>, Value, RefCell<Option<Key>>, bool)>),
}
pub(super) fn assignment_skips(op: &str, value: &Value) -> bool {
    match op {
        "&&" => !value.truthy(),
        "||" => value.truthy(),
        "??" => !matches!(value, Value::Null | Value::Undefined),
        _ => false,
    }
}
impl Engine<'_> {
    /// ResolveBinding (§9.4.2) for an IdentifierReference.
    pub(super) fn name_reference<'a>(&mut self, name: &'a str, env: Id) -> Result<Reference<'a>> {
        let strict = self.env_strict(env);
        Ok(match self.resolve_name(env, name, strict)? {
            super::vm::Resolved::Unresolvable => Reference::Unresolvable(name),
            super::vm::Resolved::With(object) => {
                self.protect(&Value::Object(object))?;
                Reference::WithBinding(object, name, strict)
            }
            super::vm::Resolved::Found(found) => Reference::Binding(found, name, strict),
        })
    }
    pub(super) fn eval_reference<'a>(
        &mut self,
        target: &'a Expr,
        env: Id,
    ) -> Result<Reference<'a>> {
        match target {
            Expr::Name(name) => self.name_reference(name, env),
            Expr::Member(base, key) => {
                let base = self.eval(base, env)?;
                self.protect(&base)?;
                // §13.3.2.1: no RequireObjectCoercible here; GetValue/PutValue's
                // ToObject throws for a nullish base, after an assignment's RHS.
                Ok(Reference::Property(
                    base,
                    Key::from(key.as_str()),
                    self.env_strict(env),
                ))
            }
            Expr::Computed(base, key) => {
                let base = self.eval(base, env)?;
                self.protect(&base)?;
                let key = self.eval(key, env)?;
                self.protect(&key)?;
                Ok(Reference::Computed(Box::new((
                    base,
                    key,
                    RefCell::new(None),
                    self.env_strict(env),
                ))))
            }
            Expr::SuperComputed(k) => {
                // §13.3.7.1 + §13.3.7.3 MakeSuperPropertyReference: GetSuperBase after
                // the expression; ToPropertyKey waits for GetValue/PutValue (§6.2.5.5,
                // §6.2.5.6), so an assignment's RHS runs first.
                let this = self.lookup(env, "this")?;
                self.protect(&this)?;
                let raw = self.eval(k, env)?;
                self.protect(&raw)?;
                let (_, target) = self.super_base(env)?;
                Ok(Reference::SuperComputed(Box::new((
                    this,
                    target,
                    raw,
                    RefCell::new(None),
                    self.env_strict(env),
                ))))
            }
            Expr::SuperMember(_) => {
                let (this, target, key) = self.super_key(target, env)?;
                Ok(Reference::Super(this, target, key, self.env_strict(env)))
            }
            _ => Err(ScriptError::Runtime("invalid reference")),
        }
    }
    pub(super) fn reference_get(&mut self, reference: &Reference) -> Result<Value> {
        match reference {
            Reference::Binding(env, name, _) => self.lookup(*env, name),
            Reference::WithBinding(object, name, strict) => {
                // §9.1.1.2.6 GetBindingValue steps 2-4.
                if !self.has(*object, *name)? {
                    if *strict {
                        return self
                            .throw_error("ReferenceError", &format!("{name} is not defined"));
                    }
                    return Ok(Value::Undefined);
                }
                self.get_member(Value::Object(*object), *name)
            }
            Reference::Unresolvable(name) => {
                self.throw_error("ReferenceError", &format!("{name} is not defined"))
            }
            Reference::Property(base, key, _) => self.get_member(base.clone(), key),
            Reference::Computed(data) => {
                let (base, raw, cached, _) = &**data;
                let existing = cached.borrow().clone();
                let key = if let Some(key) = existing {
                    key
                } else {
                    let key = self.member_key(base, raw.clone())?;
                    *cached.borrow_mut() = Some(key.clone());
                    key
                };
                self.get_member(base.clone(), key)
            }
            Reference::Super(this, target, key, _) => self.super_get(this.clone(), *target, key),
            Reference::SuperComputed(data) => {
                let (this, target, raw, cached, _) = &**data;
                let existing = cached.borrow().clone();
                let key = match existing {
                    Some(key) => key,
                    None => {
                        let key = self.to_key(raw.clone())?;
                        *cached.borrow_mut() = Some(key.clone());
                        key
                    }
                };
                self.super_get(this.clone(), *target, &key)
            }
        }
    }
    pub(super) fn reference_set(&mut self, reference: Reference, value: Value) -> Result<Value> {
        self.protect(&value)?;
        match reference {
            Reference::Binding(env, name, strict) => {
                self.assign_binding_hinted(env, name, value.clone(), None, strict)?
            }
            Reference::WithBinding(object, name, strict) => {
                // §9.1.1.2.5 SetMutableBinding steps 1-3.
                if !self.has(object, name)? && strict {
                    return self.throw_error("ReferenceError", &format!("{name} is not defined"));
                }
                self.assign_member(Value::Object(object), name.into(), value.clone(), strict)?
            }
            Reference::Unresolvable(name) => {
                return self.throw_error("ReferenceError", &format!("{name} is not defined"))
            }
            Reference::Property(base, key, strict) => {
                self.assign_member(base, key, value.clone(), strict)?
            }
            Reference::Computed(data) => {
                let (base, raw, cached, strict) = *data;
                // GetValue in compound/update expressions has already converted
                // and memoized the key. Plain assignment converts it here, after RHS.
                let key = match cached.into_inner() {
                    Some(key) => key,
                    None => self.member_key(&base, raw)?,
                };
                self.assign_member(base, key, value.clone(), strict)?
            }
            Reference::Super(this, target, key, strict) => {
                self.super_set(this, target, key, value.clone(), strict)?
            }
            Reference::SuperComputed(data) => {
                let (this, target, raw, cached, strict) = *data;
                let key = match cached.into_inner() {
                    Some(key) => key,
                    None => self.to_key(raw)?,
                };
                self.super_set(this, target, key, value.clone(), strict)?
            }
        }
        Ok(value)
    }
    pub(super) fn compound(
        &mut self,
        op: &str,
        target: &Expr,
        rhs: &Expr,
        env: Id,
    ) -> Result<Value> {
        let reference = self.eval_reference(target, env)?;
        let old = self.reference_get(&reference)?;
        self.protect(&old)?;
        if assignment_skips(op, &old) {
            return Ok(old);
        }
        // §13.15.2 steps 4.a/5.a/6.a: a logical assignment to an IdentifierReference
        // names an anonymous function definition (NamedEvaluation).
        let rhs = match target {
            Expr::Name(name) if matches!(op, "&&" | "||" | "??") => {
                self.eval_named(rhs, env, name)?
            }
            _ => self.eval(rhs, env)?,
        };
        self.protect(&rhs)?;
        let value = if matches!(op, "&&" | "||" | "??") {
            rhs
        } else {
            self.binary_values(op, old, rhs)?
        };
        self.reference_set(reference, value)
    }
    pub(super) fn update(
        &mut self,
        target: &Expr,
        increment: bool,
        prefix: bool,
        env: Id,
    ) -> Result<Value> {
        let reference = self.eval_reference(target, env)?;
        let old = self.reference_get(&reference)?;
        self.protect(&old)?;
        let old = self.to_primitive(old, false)?;
        if let Value::BigInt(value) = old {
            let next = self.big_value(if increment {
                value.add(&super::bigint::Big::from_u64(1))
            } else {
                value.sub(&super::bigint::Big::from_u64(1))
            })?;
            let next = self.reference_set(reference, next)?;
            return Ok(if prefix { next } else { Value::BigInt(value) });
        }
        let old = self.to_number(old)?;
        let next = Value::number(old + if increment { 1.0 } else { -1.0 });
        let next = self.reference_set(reference, next)?;
        Ok(if prefix { next } else { Value::number(old) })
    }
}
