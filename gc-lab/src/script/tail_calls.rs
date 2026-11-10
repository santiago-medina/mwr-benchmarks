//! Proper tail calls (ECMA-262 §15.10, design in TAIL_CALLS.md).
//!
//! The tree-walker evaluates a call by recursing on the Rust stack. A `return` whose
//! expression has a call in tail position (§15.10.2 HasCallInTailPosition) in strict
//! ordinary code (§15.10.1 IsInTailPosition) instead evaluates the callee, `this` and the
//! arguments, stores them in `Engine::tail_call` and unwinds to the caller's `invoke_this`.
//! That frame performs the call in a loop (§15.10.3 PrepareForTailCall: the running
//! execution context is gone before the call), so tail recursion runs in constant stack.
use super::parser::Expr;
use super::vm::{Engine, ScriptError, TailCall, Value};
use crate::dual::Id;

type Result<T> = std::result::Result<T, ScriptError>;

impl Engine<'_> {
    /// Evaluate the `return` expression of a body in tail position. Only the shapes
    /// §15.10.2.2 lists pass tail position down; everything else evaluates normally.
    pub(super) fn eval_tail(&mut self, x: &Expr, env: Id) -> Result<Value> {
        match x {
            // CallExpression : CoverCallExpressionAndAsyncArrowHead / CallExpression Arguments.
            // A direct eval stays an ordinary call (eval_call_in returns before deferring).
            Expr::Call(f, args) => self.eval_call_in(f, args, env, false, true),
            // MemberExpression TemplateLiteral / CallExpression TemplateLiteral.
            Expr::Template(t) if t.tag.is_some() => self.eval_template_in(t, env, true),
            // ConditionalExpression : both AssignmentExpression branches.
            Expr::Conditional(condition, yes, no) => {
                let c = self.eval(condition, env)?;
                self.eval_tail(if c.truthy() { yes } else { no }, env)
            }
            // LogicalANDExpression, LogicalORExpression, CoalesceExpression: the right operand.
            Expr::Binary(op, a, b) if matches!(op.as_str(), "&&" | "||" | "??") => {
                let a = self.eval(a, env)?;
                self.protect(&a)?;
                if super::references::assignment_skips(op, &a) {
                    return Ok(a);
                }
                self.eval_tail(b, env)
            }
            // Expression : Expression , AssignmentExpression (and ParenthesizedExpression).
            Expr::Sequence(items) if !items.is_empty() => {
                let (last, rest) = items.split_last().unwrap();
                for item in rest {
                    self.frame(|e| e.eval(item, env))?;
                }
                self.eval_tail(last, env)
            }
            _ => self.eval(x, env),
        }
    }
    /// Store the call for the trampoline. The value returned is a placeholder: the body
    /// unwinds with it and `invoke_this` replaces it with the call's result.
    pub(super) fn defer_tail_call(
        &mut self,
        callee: Value,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let mut roots = vec![];
        for value in std::iter::once(&callee)
            .chain(std::iter::once(&receiver))
            .chain(args.iter())
        {
            if let Some(id) = value.id() {
                roots.push(self.runtime.root(id)?);
            }
        }
        let previous = self.tail_call.replace(TailCall {
            callee,
            receiver,
            args,
            roots,
        });
        debug_assert!(previous.is_none(), "one deferred tail call at a time");
        if let Some(previous) = previous {
            self.release_tail_call(previous);
        }
        Ok(Value::Undefined)
    }
    /// Drop the call's roots and hand back its parts (the next frame protects them).
    pub(super) fn release_tail_call(&mut self, call: TailCall) -> (Value, Value, Vec<Value>) {
        for root in call.roots {
            self.runtime.unroot(root).expect("owned tail-call root");
        }
        (call.callee, call.receiver, call.args)
    }
    /// Run `f` with `return` out of tail position (try blocks, catch with finally,
    /// for-in/of bodies, blocks with `using` declarations).
    pub(super) fn non_tail_scope<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.non_tail += 1;
        let r = f(self);
        self.non_tail -= 1;
        r
    }
}
