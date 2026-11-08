//! A reference is evaluated once, before the value that will be assigned to it.
use super::{
    objects::Key,
    parser::Expr,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
pub(super) enum Reference {
    Binding(Id, String),
    Property(Value, Key),
    Super(Value, Option<Id>, Key),
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
    pub(super) fn eval_reference(&mut self, target: &Expr, env: Id) -> Result<Reference> {
        match target {
            Expr::Name(name) => Ok(Reference::Binding(env, name.clone())),
            Expr::Member(base, key) => {
                let base = self.eval(base, env)?;
                self.protect(&base)?;
                let key = self.member_key(&base, Value::String(key.clone()))?;
                Ok(Reference::Property(base, key))
            }
            Expr::Computed(base, key) => {
                let base = self.eval(base, env)?;
                self.protect(&base)?;
                let key = self.eval(key, env)?;
                self.protect(&key)?;
                let key = self.member_key(&base, key)?;
                Ok(Reference::Property(base, key))
            }
            Expr::SuperMember(_) | Expr::SuperComputed(_) => {
                let (this, target, key) = self.super_key(target, env)?;
                Ok(Reference::Super(this, target, key))
            }
            _ => Err(ScriptError::Runtime("invalid reference")),
        }
    }
    fn reference_get(&mut self, reference: &Reference) -> Result<Value> {
        match reference {
            Reference::Binding(env, name) => self.lookup(*env, name),
            Reference::Property(base, key) => self.get_member(base.clone(), key),
            Reference::Super(this, target, key) => self.super_get(this.clone(), *target, key),
        }
    }
    pub(super) fn reference_set(&mut self, reference: Reference, value: Value) -> Result<Value> {
        self.protect(&value)?;
        match reference {
            Reference::Binding(env, name) => self.assign_binding(env, &name, value.clone())?,
            Reference::Property(base, key) => self.set_member(base, key, value.clone())?,
            Reference::Super(this, target, key) => {
                self.super_set(this, target, key, value.clone())?
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
        let rhs = self.eval(rhs, env)?;
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
