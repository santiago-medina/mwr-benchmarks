//! A small executable JavaScript subset, not an ECMAScript-compatible engine.
mod builtins;
mod execute;
mod host;
mod objects;
pub mod parser;
mod vm;
pub use vm::{Engine, ScriptError, SuspendedEngine};

mod adapter;
pub use adapter::DomAdapter;

pub mod wire;

mod arrays;

mod promises;

mod asynchronous;

mod network;
pub use network::FetchResponse;

mod modules;
pub use modules::{ModuleSource, ModuleStatus};

mod collections;

mod array_methods;

mod json;

mod numeric;

mod strings;

mod errors;
mod promise_combinators;

mod references;

mod loops;

mod symbols;

mod array_copy;
mod object_methods;

mod dynamic_functions;
mod uri;

mod set_methods;

mod enumeration;

mod spread;

mod parameters;

mod bigint;

mod bigint_vm;

mod buffer_elements;
mod buffers;

mod typed_arrays;

mod sorting;
mod typed_methods;

mod typed_copy;
