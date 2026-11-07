//! A small executable JavaScript subset, not an ECMAScript-compatible engine.
mod host;
mod objects;
mod execute;
mod builtins;
pub mod parser;
mod vm;
pub use vm::{Engine, ScriptError, SuspendedEngine};

mod adapter;
pub use adapter::DomAdapter;

pub mod wire;
