/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Hand-written bodies of the SpiderMonkey entry points the facade implements
//! (MOZJS_FACADE.md, C1). `tools/gen_sys.py` makes the generated definition of
//! every name in `tools/implemented.txt` delegate here with the same
//! arguments, so each function below keeps the bindgen signature.
//!
//! These functions take raw pointers because the mozjs API does; the engine
//! logic stays in the safe `gc_lab::script::embed` API.

mod barriers;
mod context;
mod jobs;
mod natives;
mod objects;
mod props;
mod proxy;
mod scripts;

pub use barriers::*;
pub use context::*;
pub use jobs::*;
pub use natives::*;
pub use objects::*;
pub use props::*;
pub use proxy::*;
pub use scripts::*;
