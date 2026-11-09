//! Template site records are immutable AST data; cached arrays belong to their realm graph.
use super::{
    objects::Descriptor,
    parser::Expr,
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::Id;
use std::sync::atomic::{AtomicU64, Ordering};
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub site: u64,
    pub parts: Vec<(Option<String>, String)>,
    pub values: Vec<Expr>,
    pub tag: Option<Box<Expr>>,
}
pub(super) fn site() -> std::result::Result<u64, ()> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| ())
}
impl Engine<'_> {
    pub(super) fn template_object(&mut self, t: &Template, mut env: Id) -> Result<Value> {
        if let Some(id) = self.template_cache.get(&t.site).copied() {
            return Ok(Value::Object(id));
        }
        if self.template_cache.len() >= 8192 {
            return Err(ScriptError::Runtime("template site limit"));
        }
        let raw = self.new_array(t.parts.len() as u32)?;
        self.protect(&Value::Object(raw))?;
        let cooked = self.new_array(t.parts.len() as u32)?;
        self.protect(&Value::Object(cooked))?;
        for (i, (text, source)) in t.parts.iter().enumerate() {
            self.tick()?;
            for (id, value) in [
                (raw, Value::String(source.clone())),
                (
                    cooked,
                    text.clone().map(Value::String).unwrap_or(Value::Undefined),
                ),
            ] {
                self.define(
                    id,
                    i.to_string(),
                    Descriptor {
                        value: Some(value),
                        writable: Some(false),
                        enumerable: Some(true),
                        configurable: Some(false),
                        ..Default::default()
                    },
                )?;
            }
        }
        self.define(
            cooked,
            "raw",
            Descriptor {
                value: Some(Value::Object(raw)),
                ..Default::default()
            },
        )?;
        for id in [raw, cooked] {
            self.freeze_template(id)?;
        }
        loop {
            self.tick()?;
            match self.objects.get(&env) {
                Some(Object::Environment {
                    parent: Some(parent),
                    ..
                }) => env = *parent,
                Some(Object::Environment { parent: None, .. }) => break,
                _ => return Err(ScriptError::Runtime("invalid template realm environment")),
            }
        }
        self.runtime.link(env, cooked)?;
        self.template_cache.insert(t.site, cooked);
        Ok(Value::Object(cooked))
    }
    fn freeze_template(&mut self, id: Id) -> Result<()> {
        self.define(
            id,
            "length",
            Descriptor {
                writable: Some(false),
                ..Default::default()
            },
        )?;
        let mut data = self.data(id)?.clone();
        data.extensible = false;
        self.replace_data(id, data)
    }
    pub(super) fn eval_template(&mut self, t: &Template, env: Id) -> Result<Value> {
        if let Some(tag) = &t.tag {
            let (receiver, callee) = self.call_target(tag, env)?;
            self.protect(&receiver)?;
            self.protect(&callee)?;
            let mut args = vec![self.template_object(t, env)?];
            self.protect(&args[0])?;
            for expression in &t.values {
                let value = self.eval(expression, env)?;
                self.protect(&value)?;
                args.push(value);
            }
            self.invoke_this(callee, receiver, args)
        } else {
            let mut text = t.parts[0]
                .0
                .clone()
                .ok_or(ScriptError::Runtime("invalid untagged template escape"))?;
            for (i, expression) in t.values.iter().enumerate() {
                let value = self.eval(expression, env)?;
                self.protect(&value)?;
                let part = t.parts[i + 1]
                    .0
                    .as_deref()
                    .ok_or(ScriptError::Runtime("invalid untagged template escape"))?;
                text = self.template_text(text, value, part)?;
            }
            Ok(Value::String(text))
        }
    }
    pub(super) fn template_text(
        &mut self,
        mut text: String,
        value: Value,
        part: &str,
    ) -> Result<String> {
        let substitution = self.to_string(value)?;
        if text
            .len()
            .saturating_add(substitution.len())
            .saturating_add(part.len())
            > 16_777_216
        {
            return Err(ScriptError::Runtime("template string limit"));
        }
        text.try_reserve(substitution.len() + part.len())
            .map_err(|_| ScriptError::Runtime("template allocation failed"))?;
        text.push_str(&substitution);
        text.push_str(part);
        Ok(text)
    }
    pub(super) fn string_raw(&mut self, args: Vec<Value>) -> Result<Value> {
        let template = self.object_coercible(args.first().cloned().unwrap_or(Value::Undefined))?;
        self.protect(&Value::Object(template))?;
        let raw = self.get_member(Value::Object(template), "raw")?;
        self.protect(&raw)?;
        let raw = self.object_coercible(raw)?;
        self.protect(&Value::Object(raw))?;
        let length = self.length_of(raw)?;
        let mut text = String::new();
        for index in 0..length {
            self.tick()?;
            let part = self.get_member(Value::Object(raw), index.to_string())?;
            self.protect(&part)?;
            text = self.template_text(text, part, "")?;
            if index + 1 < length {
                let substitution = args
                    .get(index as usize + 1)
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                text = self.template_text(text, substitution, "")?;
            }
        }
        Ok(Value::String(text))
    }
    pub(super) fn call_target(&mut self, callee: &Expr, env: Id) -> Result<(Value, Value)> {
        match callee {
            Expr::Member(base, key) => {
                let receiver = self.eval(base, env)?;
                self.protect(&receiver)?;
                let function = self.get_member(receiver.clone(), key)?;
                Ok((receiver, function))
            }
            Expr::Computed(base, key) => {
                let receiver = self.eval(base, env)?;
                self.protect(&receiver)?;
                let key = self.eval(key, env)?;
                self.protect(&key)?;
                let key = self.member_key(&receiver, key)?;
                let function = self.get_member(receiver.clone(), key)?;
                Ok((receiver, function))
            }
            Expr::SuperMember(_) | Expr::SuperComputed(_) => {
                let (this, target, key) = self.super_key(callee, env)?;
                let function = self.super_get(this.clone(), target, &key)?;
                Ok((this, function))
            }
            Expr::Name(name) => {
                // A call through a `with` object reference gets the object as `this`.
                let receiver = match self.with_base(env, name)? {
                    Some(object) => Value::Object(object),
                    None => Value::Undefined,
                };
                Ok((receiver, self.eval(callee, env)?))
            }
            _ => Ok((Value::Undefined, self.eval(callee, env)?)),
        }
    }
}
