//! Native adapter is installed in the next development step.
use super::vm::{Engine, ScriptError, Value};
use crate::dual::Id;
impl Engine {
    pub(super) fn get_member(&mut self, _: Value, _: &str) -> Result<Value, ScriptError> {
        Err(ScriptError::Runtime("native adapter not installed"))
    }
    pub(super) fn set_member(&mut self, _: Value, _: &str, _: Value) -> Result<(), ScriptError> {
        Err(ScriptError::Runtime("native adapter not installed"))
    }
    pub(super) fn call_native(
        &mut self,
        _: Option<Id>,
        _: &str,
        _: Vec<Value>,
    ) -> Result<Value, ScriptError> {
        Err(ScriptError::Runtime("native adapter not installed"))
    }
}
