//! A small executable JavaScript subset, not an ECMAScript-compatible engine.
mod host;
pub mod parser;
mod vm;
pub use vm::{Engine, ScriptError, SuspendedEngine};

mod adapter;
pub use adapter::DomAdapter;

pub mod wire;
