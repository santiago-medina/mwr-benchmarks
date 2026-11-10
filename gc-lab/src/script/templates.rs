//! Template site records are immutable AST data; cached arrays belong to their realm graph.
use super::js_string::{Builder, JsString};
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
        // [[TemplateMap]] is per realm (§13.2.8.4 GetTemplateObject): sites are a counter,
        // so the realm index goes in the high bits of the key.
        let key = t.site | ((self.current_realm as u64) << 48);
        if let Some(id) = self.template_cache.get(&key).copied() {
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
                (raw, Value::String(source.clone().into())),
                (
                    cooked,
                    text.clone()
                        .map(|s| Value::String(s.into()))
                        .unwrap_or(Value::Undefined),
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
        self.template_cache.insert(key, cooked);
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
        self.eval_template_in(t, env, false)
    }
    /// `tail`: the tagged call is in tail position (§15.10.2, MemberExpression TemplateLiteral).
    pub(super) fn eval_template_in(&mut self, t: &Template, env: Id, tail: bool) -> Result<Value> {
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
            if tail {
                return self.defer_tail_call(callee, receiver, args);
            }
            self.invoke_this(callee, receiver, args)
        } else {
            let mut text: JsString = t.parts[0]
                .0
                .as_deref()
                .ok_or(ScriptError::Runtime("invalid untagged template escape"))?
                .into();
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
    /// One TemplateLiteral step (§13.2.8.6): `text` + ToString(value) + `part`.
    pub(super) fn template_text(
        &mut self,
        text: JsString,
        value: Value,
        part: &str,
    ) -> Result<JsString> {
        let substitution = self.to_js_string(value)?;
        if text
            .size()
            .saturating_add(substitution.size())
            .saturating_add(part.len())
            > 16_777_216
        {
            return Err(ScriptError::Runtime("template string limit"));
        }
        let mut out = Builder::default();
        out.push(&text);
        out.push(&substitution);
        out.push_str(part);
        Ok(out.finish())
    }
    pub(super) fn string_raw(&mut self, args: Vec<Value>) -> Result<Value> {
        let template = self.object_coercible(args.first().cloned().unwrap_or(Value::Undefined))?;
        self.protect(&Value::Object(template))?;
        let raw = self.get_member(Value::Object(template), "raw")?;
        self.protect(&raw)?;
        let raw = self.object_coercible(raw)?;
        self.protect(&Value::Object(raw))?;
        let length = self.length_of(raw)?;
        let mut text = JsString::default();
        for index in 0..length {
            self.tick()?;
            let part = self.get_member(Value::Object(raw), index.to_string())?;
            self.protect(&part)?;
            text = self.template_text(text, part, "")?;
            if index + 1 < length {
                let substitution = args
                    .get(index as usize + 1)
                    .cloned()
                    .unwrap_or(Value::String(String::new().into()));
                text = self.template_text(text, substitution, "")?;
            }
        }
        Ok(Value::String(text))
    }
    pub(super) fn call_target(&mut self, callee: &Expr, env: Id) -> Result<(Value, Value)> {
        match callee {
            // `a.b?.()`: the optional link keeps the member's `this` (§13.3.9.2).
            Expr::Optional(base) => {
                let (receiver, function) = self.call_target(base, env)?;
                if matches!(function, Value::Undefined | Value::Null) {
                    return Err(ScriptError::ShortCircuit);
                }
                Ok((receiver, function))
            }
            // `(a?.b)()`: a parenthesized chain still yields a Reference (§13.2.9.1).
            Expr::OptionalChain(chain) => match self.call_target(chain, env) {
                Err(ScriptError::ShortCircuit) => Ok((Value::Undefined, Value::Undefined)),
                other => other,
            },
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
                // One ResolveBinding for both the value and the `this` (§13.3.6.1).
                let (function, base) = self.lookup_with_base(env, name)?;
                let receiver = match base {
                    Some(object) => Value::Object(object),
                    None => Value::Undefined,
                };
                Ok((receiver, function))
            }
            _ => Ok((Value::Undefined, self.eval(callee, env)?)),
        }
    }
}
