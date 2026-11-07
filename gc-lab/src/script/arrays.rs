//! Array exotic operations and iterator state share ordinary property storage.
use crate::dual::Id;
#[derive(Clone)]
pub(super) struct IteratorState {pub target:Option<Id>,pub index:u64,pub kind:u8}
