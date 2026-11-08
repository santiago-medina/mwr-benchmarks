//! Binding patterns share one initialization path and the existing iterator/property protocols.
use super::{
    objects::Key,
    parser::Expr,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternKey {
    Static(String),
    Computed(Expr),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    Name(String),
    Array(Vec<Option<Pattern>>, Option<Box<Pattern>>),
    Object(Vec<(PatternKey, Pattern)>, Option<Box<Pattern>>),
    Default(Box<Pattern>, Expr),
}
impl Pattern {
    pub fn names(&self) -> Vec<String> {
        let mut names = vec![];
        self.collect_names(&mut names);
        names
    }
    fn collect_names(&self, names: &mut Vec<String>) {
        match self {
            Self::Name(name) => names.push(name.clone()),
            Self::Array(items, rest) => {
                for item in items.iter().flatten() {
                    item.collect_names(names);
                }
                if let Some(rest) = rest {
                    rest.collect_names(names);
                }
            }
            Self::Object(properties, rest) => {
                for (_, pattern) in properties {
                    pattern.collect_names(names);
                }
                if let Some(rest) = rest {
                    rest.collect_names(names);
                }
            }
            Self::Default(pattern, _) => pattern.collect_names(names),
        }
    }
    pub fn expressions(&self) -> Vec<&Expr> {
        let mut expressions = vec![];
        self.collect_expressions(&mut expressions);
        expressions
    }
    fn collect_expressions<'a>(&'a self, expressions: &mut Vec<&'a Expr>) {
        match self {
            Self::Name(_) => {}
            Self::Array(items, rest) => {
                for item in items.iter().flatten() {
                    item.collect_expressions(expressions);
                }
                if let Some(rest) = rest {
                    rest.collect_expressions(expressions);
                }
            }
            Self::Object(properties, rest) => {
                for (key, pattern) in properties {
                    if let PatternKey::Computed(expression) = key {
                        expressions.push(expression);
                    }
                    pattern.collect_expressions(expressions);
                }
                if let Some(rest) = rest {
                    rest.collect_expressions(expressions);
                }
            }
            Self::Default(pattern, expression) => {
                expressions.push(expression);
                pattern.collect_expressions(expressions);
            }
        }
    }
}
impl Engine<'_> {
    fn binding_step(&mut self, iterator: Value, next: Value, done: &mut bool) -> Result<Value> {
        if *done {
            return Ok(Value::Undefined);
        }
        let outcome = (|| {
            let step = self.invoke_this(next, iterator, vec![])?;
            self.protect(&step)?;
            if !matches!(step, Value::Object(_)) {
                return self.throw_error("TypeError", "binding iterator result is not an object");
            }
            if self.get_member(step.clone(), "done")?.truthy() {
                *done = true;
                return Ok(Value::Undefined);
            }
            self.get_member(step, "value")
        })();
        if outcome.is_err() {
            *done = true;
        }
        outcome
    }
    pub(super) fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        value: Value,
        env: Id,
        target: Id,
    ) -> Result<()> {
        self.protect(&value)?;
        match pattern {
            Pattern::Name(name) => self.initialize(target, name, value),
            Pattern::Default(pattern, default) => {
                let value = if value == Value::Undefined {
                    self.eval(default, env)?
                } else {
                    value
                };
                self.bind_pattern(pattern, value, env, target)
            }
            Pattern::Object(properties, rest) => {
                let source = self.object_coercible(value)?;
                self.protect(&Value::Object(source))?;
                let mut excluded = vec![];
                for (key, pattern) in properties {
                    self.tick()?;
                    self.frame(|e| {
                        let key = match key {
                            PatternKey::Static(key) => Key::from(key),
                            PatternKey::Computed(expression) => {
                                let value = e.eval(expression, env)?;
                                e.protect(&value)?;
                                e.to_key(value)?
                            }
                        };
                        excluded.push(key.clone());
                        let value = e.get_member(Value::Object(source), key)?;
                        e.bind_pattern(pattern, value, env, target)
                    })?;
                }
                if let Some(rest) = rest {
                    let id = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
                    self.protect(&Value::Object(id))?;
                    self.copy_properties(id, Value::Object(source), &excluded)?;
                    self.bind_pattern(rest, Value::Object(id), env, target)?;
                }
                Ok(())
            }
            Pattern::Array(items, rest) => {
                let method = self.get_member(value.clone(), Key::Symbol(0))?;
                self.protect(&method)?;
                let (iterator, next) = self.open_iterator(value, method)?;
                let mut done = false;
                let outcome = (|| {
                    for pattern in items {
                        self.tick()?;
                        self.frame(|e| {
                            let value =
                                e.binding_step(iterator.clone(), next.clone(), &mut done)?;
                            e.protect(&value)?;
                            if let Some(pattern) = pattern {
                                e.bind_pattern(pattern, value, env, target)?;
                            }
                            Ok(())
                        })?;
                    }
                    if let Some(rest) = rest {
                        let array = self.new_array(0)?;
                        self.protect(&Value::Object(array))?;
                        while !done {
                            self.tick()?;
                            self.frame(|e| {
                                let value =
                                    e.binding_step(iterator.clone(), next.clone(), &mut done)?;
                                if !done {
                                    e.append_array(array, Some(value), false)?;
                                }
                                Ok(())
                            })?;
                        }
                        self.bind_pattern(rest, Value::Object(array), env, target)?;
                    }
                    Ok(())
                })();
                if done {
                    return outcome;
                }
                self.close_iterator(iterator, outcome.map(|_| None))
                    .map(|_| ())
            }
        }
    }
}
