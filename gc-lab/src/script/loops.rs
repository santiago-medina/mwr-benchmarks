//! Loop scopes share environment records; captures retain a distinct let binding per iteration.
use super::{
    parser::{Expr, Stmt},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::Id;
type Result<T> = std::result::Result<T, ScriptError>;
impl Engine<'_> {
    pub(super) fn switch_statement(
        &mut self,
        expression: &Expr,
        cases: &[(Option<Expr>, Vec<Stmt>)],
        env: Id,
        can_return: bool,
    ) -> Result<Option<Value>> {
        let value = self.eval(expression, env)?;
        self.protect(&value)?;
        let all: Vec<Stmt> = cases
            .iter()
            .flat_map(|(_, body)| body.iter().cloned())
            .collect();
        let local = self.new_environment(Some(env), &all, false)?;
        self.protect(&Value::Object(local))?;
        self.hoist_functions(&all, local)?;
        let mut selected = None;
        let mut default = None;
        for (index, (test, _)) in cases.iter().enumerate() {
            if let Some(test) = test {
                let other = self.frame(|e| e.eval(test, local))?;
                if value.strict_equal(&other) {
                    selected = Some(index);
                    break;
                }
            } else {
                default = Some(index);
            }
        }
        if let Some(start) = selected.or(default) {
            for (_, body) in &cases[start..] {
                for statement in body {
                    self.tick()?;
                    match self.frame(|e| e.statement(statement, local, can_return)) {
                        Ok(None) => {}
                        Err(ScriptError::LoopBreak) => return Ok(None),
                        other => return other,
                    }
                }
            }
        }
        Ok(None)
    }
    pub(super) fn clone_iteration(&mut self, env: Id, names: &[String]) -> Result<Id> {
        let parent = match self.objects.get(&env) {
            Some(Object::Environment { parent, .. }) => *parent,
            _ => return Err(ScriptError::Runtime("invalid loop environment")),
        };
        let body: Vec<_> = names
            .iter()
            .map(|n| Stmt::Let(n.clone(), Expr::Undefined))
            .collect();
        let next = self.new_environment(parent, &body, false)?;
        self.protect(&Value::Object(next))?;
        for name in names {
            let value = self.lookup(env, name)?;
            self.initialize(next, name, value)?;
        }
        Ok(next)
    }
    pub(super) fn while_loop(
        &mut self,
        test: &Expr,
        body: &Stmt,
        post: bool,
        env: Id,
        can_return: bool,
    ) -> Result<Option<Value>> {
        let mut first = true;
        loop {
            self.tick()?;
            if !(post && first) && !self.frame(|e| e.eval(test, env).map(|v| v.truthy()))? {
                return Ok(None);
            }
            first = false;
            match self.frame(|e| e.statement(body, env, can_return)) {
                Ok(None) | Err(ScriptError::LoopContinue) => {}
                Err(ScriptError::LoopBreak) => return Ok(None),
                other => return other,
            }
        }
    }
    pub(super) fn for_loop(
        &mut self,
        init: Option<&Stmt>,
        test: Option<&Expr>,
        update: Option<&Expr>,
        body: &Stmt,
        env: Id,
        can_return: bool,
    ) -> Result<Option<Value>> {
        let lexical = init.is_some_and(|s| {
            matches!(s, Stmt::Let(..) | Stmt::Const(..))
                || matches!(s,Stmt::Bind{kind,..} if kind!="var")
        });
        let mut local = if lexical {
            let id = self.new_environment(Some(env), std::slice::from_ref(init.unwrap()), false)?;
            self.protect(&Value::Object(id))?;
            id
        } else {
            env
        };
        if let Some(init) = init {
            self.statement(init, local, can_return)?;
        }
        let names = match init {
            Some(Stmt::Let(name, _)) => vec![name.clone()],
            Some(Stmt::Bind { kind, pattern, .. }) if kind == "let" => pattern.names(),
            _ => vec![],
        };
        if !names.is_empty() {
            local = self.clone_iteration(local, &names)?;
        }
        let carrier = self.new_object(None)?;
        self.protect(&Value::Object(carrier))?;
        self.put_data(carrier, "scope", Value::Object(local))?;
        loop {
            self.tick()?;
            let outcome = self.frame(|e| {
                if let Some(test) = test {
                    if !e.eval(test, local)?.truthy() {
                        return Ok(Some(None));
                    }
                }
                match e.statement(body, local, can_return) {
                    Ok(None) | Err(ScriptError::LoopContinue) => {}
                    Err(ScriptError::LoopBreak) => return Ok(Some(None)),
                    Ok(Some(value)) => return Ok(Some(Some(value))),
                    Err(error) => return Err(error),
                }
                if !names.is_empty() {
                    local = e.clone_iteration(local, &names)?;
                    e.put_data(carrier, "scope", Value::Object(local))?;
                }
                if let Some(update) = update {
                    e.eval(update, local)?;
                }
                Ok(None)
            })?;
            if let Some(result) = outcome {
                return Ok(result);
            }
        }
    }
}
