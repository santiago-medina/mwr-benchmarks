//! Async continuations; no borrowed Engine or host context survives suspension.
use super::{vm::{Engine,Value,FunctionKind,ScriptError},parser::Stmt};
use crate::dual::Id;
use std::sync::Arc;
pub(super) struct Program;
pub(super) fn compile(_body:&[Stmt])->Result<Arc<Program>,ScriptError>{Ok(Arc::new(Program))}
impl Engine<'_>{
 pub(super) fn start_async(&mut self,_env:Id,_params:Vec<String>,_body:Vec<Stmt>,_kind:FunctionKind,_receiver:Value,_args:Vec<Value>,_program:Arc<Program>)->Result<Value,ScriptError>{Err(ScriptError::Unsupported("async execution pending"))}
}
