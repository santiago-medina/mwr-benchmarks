//! A small executable JavaScript subset, not an ECMAScript-compatible engine.
mod builtins;
mod classes;
mod execute;
mod host;
mod iterator_helpers;
mod objects;
pub mod parser;
mod props;
mod vm;
pub use vm::{Engine, GcPolicy, ScriptError, SuspendedEngine};

mod adapter;
pub use adapter::DomAdapter;

pub mod wire;

mod arrays;
mod ast_scan;

mod promises;

mod asynchronous;

mod network;
pub use network::FetchResponse;

mod modules;
pub use modules::{ModuleLoad, ModuleRequest, ModuleSource, ModuleStatus, Referrer};
pub use parser::ImportPhase;

mod bytecode;
mod collections;
mod disposal;
mod global_names;
mod interp;
mod shadow_realms;
mod tail_calls;
pub use interp::{EngineMode, VmStats};
mod weak;

mod array_methods;

mod json;

mod number_format;
mod numeric;

mod js_string;
mod normalization;
mod normalization_tables;
mod string_search;
mod strings;

mod base64;
mod errors;
mod from_async;
mod promise_combinators;

mod references;

mod loops;

mod symbols;

mod array_copy;
mod object_methods;

mod dynamic_functions;
mod eval;
mod uri;

mod set_methods;

mod enumeration;

mod spread;

mod parameters;

mod bigint;

mod bigint_vm;

mod atomics;
mod buffer_elements;
mod buffers;

mod typed_arrays;

mod regexp;
mod regexp_tables;
mod sorting;
mod typed_methods;

mod typed_copy;

mod generators;

mod delegation;

mod calendar;

mod dates;
mod regexp_builtins;

mod proxy;
mod realms;
#[cfg(feature = "temporal")]
mod temporal;

mod patterns;

mod identifiers;
mod template_lex;
mod templates;
mod unicode_id;
