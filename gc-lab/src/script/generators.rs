//! Generator state and suspended frames are traced records; resumption shares the async VM.
use super::{
    asynchronous::{Program, Target},
    objects::{Descriptor, Key},
    parser::{Param, Stmt},
    promises::{Capability, Internal},
    vm::{Engine, FunctionKind, ScriptError, Value},
};
use crate::dual::Id;
use std::{collections::HashMap, sync::Arc};
type Result<T> = std::result::Result<T, ScriptError>;
const ASYNC_ITERATOR: u64 = 12;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Start,
    Yield,
    Executing,
    Completed,
    AwaitingReturn,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Resume {
    Next,
    Throw,
    Return,
}
#[derive(Clone)]
pub(super) struct Generator {
    pub state: State,
    pub activation: Option<Id>,
    pub output: Option<std::result::Result<Value, Value>>,
    pub asynchronous: bool,
    pub queue: Vec<Request>,
}
/// AsyncGeneratorRequest: a pending next/throw/return and the promise it settles.
#[derive(Clone)]
pub(super) struct Request {
    pub mode: Resume,
    pub value: Value,
    pub capability: Capability,
}
impl Generator {
    pub fn edges(&self) -> Vec<Id> {
        let mut ids: Vec<_> = self.activation.into_iter().collect();
        for request in &self.queue {
            ids.extend(request.value.id());
            ids.extend(request.capability.values().iter().filter_map(Value::id));
        }
        if let Some(output) = &self.output {
            let value = match output {
                Ok(value) | Err(value) => value,
            };
            ids.extend(value.id());
        }
        ids
    }
}
impl Engine<'_> {
    pub(super) fn generator_output(
        &mut self,
        id: Id,
        value: Value,
        reject: bool,
        done: bool,
    ) -> Result<()> {
        self.protect(&value)?;
        let output = if reject {
            Err(value)
        } else {
            Ok(self.iterator_result(value, done)?)
        };
        let mut data = self.data(id)?.clone();
        let generator = data.generator.as_mut().unwrap();
        generator.state = if done { State::Completed } else { State::Yield };
        if done {
            generator.activation = None;
        }
        generator.output = Some(output);
        self.replace_data(id, data)
    }
    pub(super) fn start_generator(
        &mut self,
        function: Id,
        env: Id,
        params: Vec<Param>,
        body: Vec<Stmt>,
        kind: FunctionKind,
        receiver: Value,
        args: Vec<Value>,
        program: Arc<Program>,
    ) -> Result<Value> {
        let local =
            self.function_environment(function, env, &params, &body, kind, receiver, &args)?;
        let prototype = self.get_member(Value::Object(function), "prototype")?;
        let asynchronous = matches!(
            kind,
            FunctionKind::AsyncGenerator | FunctionKind::AsyncGeneratorMethod
        );
        let prototype = if let Value::Object(id) = prototype {
            id
        } else {
            self.intrinsics.as_ref().unwrap().collection[if asynchronous {
                "AsyncGenerator"
            } else {
                "Generator"
            }]
        };
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.generator = Some(Generator {
            state: State::Start,
            activation: None,
            output: None,
            asynchronous,
            queue: vec![],
        });
        self.replace_data(id, data)?;
        let target = if asynchronous {
            Target::AsyncGenerator(id)
        } else {
            Target::Generator(id)
        };
        let activation = self.new_activation(local, program, target)?;
        let mut data = self.data(id)?.clone();
        data.generator.as_mut().unwrap().activation = Some(activation);
        self.replace_data(id, data)?;
        Ok(Value::Object(id))
    }
    pub(super) fn generator_resume(
        &mut self,
        receiver: Value,
        input: Value,
        mode: Resume,
    ) -> Result<Value> {
        let Value::Object(id) = receiver else {
            return self.throw_error("TypeError", "invalid generator receiver");
        };
        let Some(generator) = self
            .data(id)
            .ok()
            .and_then(|d| d.generator.clone())
            .filter(|g| !g.asynchronous)
        else {
            return self.throw_error("TypeError", "invalid generator receiver");
        };
        if generator.state == State::Executing {
            return self.throw_error("TypeError", "generator is already executing");
        }
        if generator.state == State::Completed
            || generator.state == State::Start && mode != Resume::Next
        {
            let mut data = self.data(id)?.clone();
            let generator = data.generator.as_mut().unwrap();
            generator.state = State::Completed;
            generator.activation = None;
            generator.output = None;
            self.replace_data(id, data)?;
            return match mode {
                Resume::Throw => Err(self.thrown(input)),
                Resume::Return => self.iterator_result(input, true),
                Resume::Next => self.iterator_result(Value::Undefined, true),
            };
        }
        let activation = generator.activation.unwrap();
        let mut data = self.data(id)?.clone();
        let state = data.generator.as_mut().unwrap();
        state.state = State::Executing;
        state.output = None;
        self.replace_data(id, data)?;
        if let Err(error) = self.resume_program(activation, input, mode) {
            let mut data = self.data(id)?.clone();
            let state = data.generator.as_mut().unwrap();
            state.state = State::Completed;
            state.activation = None;
            self.replace_data(id, data)?;
            return Err(error);
        }
        let mut data = self.data(id)?.clone();
        let result = data
            .generator
            .as_mut()
            .unwrap()
            .output
            .take()
            .ok_or(ScriptError::Runtime(
                "generator resumed without a completion",
            ))?;
        self.protect(match &result {
            Ok(value) | Err(value) => value,
        })?;
        self.replace_data(id, data)?;
        match result {
            Ok(value) => Ok(value),
            Err(value) => Err(self.thrown(value)),
        }
    }
    pub(super) fn bootstrap_generators(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
        iterator: Id,
    ) -> Result<()> {
        let function = self.intrinsics.as_ref().unwrap().function;
        let function_proto = self.new_object(Some(function))?;
        self.runtime.link(env, function_proto)?;
        let generator_proto = self.new_object(Some(iterator))?;
        self.runtime.link(env, generator_proto)?;
        let ctor = self.intrinsic_function(env, "GeneratorFunction", Some(function))?;
        self.intrinsics
            .as_mut()
            .unwrap()
            .functions
            .insert("GeneratorFunction".into(), ctor);
        objects.insert("GeneratorFunction.prototype".into(), function_proto);
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("GeneratorFunction".into(), function_proto);
        self.intrinsics
            .as_mut()
            .unwrap()
            .collection
            .insert("Generator".into(), generator_proto);
        self.define(
            function_proto,
            "prototype",
            Descriptor {
                value: Some(Value::Object(generator_proto)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        self.define(
            function_proto,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("GeneratorFunction".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        self.define(
            generator_proto,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Generator".into())),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        self.hidden(
            generator_proto,
            "constructor",
            Value::Object(function_proto),
        )?;
        for method in ["next", "return", "throw"] {
            let full = format!("%Generator%.prototype.{method}");
            let f = self.intrinsic_function(env, &full, Some(function))?;
            self.hidden(generator_proto, method, Value::Object(f))?;
        }
        self.bootstrap_async_generators(env, objects)
    }
    fn bootstrap_async_generators(
        &mut self,
        env: Id,
        objects: &mut HashMap<String, Id>,
    ) -> Result<()> {
        let object = self.intrinsics.as_ref().unwrap().object;
        let function = self.intrinsics.as_ref().unwrap().function;
        let iterator_proto = self.new_object(Some(object))?;
        self.runtime.link(env, iterator_proto)?;
        let f = self.intrinsic_function(
            env,
            "%AsyncIteratorPrototype%.asyncIterator",
            Some(function),
        )?;
        self.define(
            iterator_proto,
            Key::Symbol(ASYNC_ITERATOR),
            Descriptor {
                value: Some(Value::Object(f)),
                writable: Some(true),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        let generator_proto = self.new_object(Some(iterator_proto))?;
        self.runtime.link(env, generator_proto)?;
        let function_proto = self.new_object(Some(function))?;
        self.runtime.link(env, function_proto)?;
        let ctor = self.intrinsic_function(env, "AsyncGeneratorFunction", Some(function))?;
        let intrinsics = self.intrinsics.as_mut().unwrap();
        intrinsics
            .functions
            .insert("AsyncGeneratorFunction".into(), ctor);
        intrinsics
            .collection
            .insert("AsyncGeneratorFunction".into(), function_proto);
        intrinsics
            .collection
            .insert("AsyncGenerator".into(), generator_proto);
        intrinsics
            .collection
            .insert("AsyncIterator".into(), iterator_proto);
        objects.insert("AsyncGeneratorFunction.prototype".into(), function_proto);
        self.define(
            function_proto,
            "prototype",
            Descriptor {
                value: Some(Value::Object(generator_proto)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        for (owner, tag) in [
            (function_proto, "AsyncGeneratorFunction"),
            (generator_proto, "AsyncGenerator"),
        ] {
            self.define(
                owner,
                Key::Symbol(1),
                Descriptor {
                    value: Some(Value::String(tag.into())),
                    configurable: Some(true),
                    ..Default::default()
                },
            )?;
        }
        self.define(
            generator_proto,
            "constructor",
            Descriptor {
                value: Some(Value::Object(function_proto)),
                configurable: Some(true),
                ..Default::default()
            },
        )?;
        for method in ["next", "return", "throw"] {
            let full = format!("%AsyncGenerator%.prototype.{method}");
            let f = self.intrinsic_function(env, &full, Some(function))?;
            self.hidden(generator_proto, method, Value::Object(f))?;
        }
        Ok(())
    }
    pub(super) fn async_generator_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let mode = match name.rsplit('.').next().unwrap() {
            "next" => Resume::Next,
            "return" => Resume::Return,
            _ => Resume::Throw,
        };
        let value = args.first().cloned().unwrap_or(Value::Undefined);
        self.protect(&value)?;
        let cap = self.capability(Value::Object(
            self.intrinsics.as_ref().unwrap().functions["Promise"],
        ))?;
        let target = match receiver {
            Value::Object(id)
                if self
                    .data(id)
                    .ok()
                    .and_then(|d| d.generator.as_ref().map(|g| g.asynchronous))
                    == Some(true) =>
            {
                id
            }
            _ => {
                let error = self.error_value("TypeError", "invalid async generator receiver")?;
                self.invoke(cap.reject.clone(), vec![error])?;
                return Ok(cap.promise);
            }
        };
        let mut data = self.data(target)?.clone();
        data.generator.as_mut().unwrap().queue.push(Request {
            mode,
            value,
            capability: cap.clone(),
        });
        self.replace_data(target, data)?;
        self.async_generator_drain(target)?;
        Ok(cap.promise)
    }
    fn set_generator_state(&mut self, id: Id, state: State) -> Result<()> {
        let mut data = self.data(id)?.clone();
        let generator = data.generator.as_mut().unwrap();
        generator.state = state;
        if state == State::Completed {
            generator.activation = None;
        }
        self.replace_data(id, data)
    }
    /// Pops the front request and settles its promise (AsyncGeneratorCompleteStep).
    pub(super) fn async_generator_settle(
        &mut self,
        id: Id,
        value: Value,
        reject: bool,
        done: bool,
    ) -> Result<()> {
        self.protect(&value)?;
        let mut data = self.data(id)?.clone();
        let generator = data.generator.as_mut().unwrap();
        if done {
            generator.state = State::Completed;
            generator.activation = None;
        } else {
            generator.state = State::Yield;
        }
        let request = if generator.queue.is_empty() {
            None
        } else {
            Some(generator.queue.remove(0))
        };
        self.replace_data(id, data)?;
        let Some(request) = request else {
            return Ok(());
        };
        for v in request.capability.values() {
            self.protect(&v)?;
        }
        if reject {
            self.invoke(request.capability.reject, vec![value])?;
        } else {
            let result = self.iterator_result(value, done)?;
            self.invoke(request.capability.resolve, vec![result])?;
        }
        Ok(())
    }
    pub(super) fn async_generator_awaited_return(
        &mut self,
        id: Id,
        value: Value,
        reject: bool,
    ) -> Result<()> {
        self.async_generator_settle(id, value, reject, true)?;
        self.async_generator_drain(id)
    }
    /// AsyncGeneratorResumeNext as a loop: runs queued requests until the generator suspends.
    pub(super) fn async_generator_drain(&mut self, id: Id) -> Result<()> {
        loop {
            let Some(generator) = self.data(id)?.generator.clone() else {
                return Ok(());
            };
            if matches!(generator.state, State::Executing | State::AwaitingReturn) {
                return Ok(());
            }
            let Some(request) = generator.queue.first().cloned() else {
                return Ok(());
            };
            let mut state = generator.state;
            if request.mode != Resume::Next {
                if state == State::Start {
                    state = State::Completed;
                    self.set_generator_state(id, state)?;
                }
                if state == State::Completed {
                    if request.mode == Resume::Throw {
                        self.async_generator_settle(id, request.value, true, true)?;
                        continue;
                    }
                    self.set_generator_state(id, State::AwaitingReturn)?;
                    let promise = match self.promise_resolve(
                        Value::Object(self.intrinsics.as_ref().unwrap().functions["Promise"]),
                        request.value,
                    ) {
                        Ok(promise) => promise,
                        Err(ScriptError::Thrown(error)) => {
                            self.async_generator_settle(id, error.value, true, true)?;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    self.protect(&promise)?;
                    let yes = self.internal_function(
                        Internal::AsyncGeneratorReturn {
                            generator: id,
                            reject: false,
                        },
                        1,
                    )?;
                    let no = self.internal_function(
                        Internal::AsyncGeneratorReturn {
                            generator: id,
                            reject: true,
                        },
                        1,
                    )?;
                    let Value::Object(p) = promise else {
                        unreachable!()
                    };
                    self.attach_reaction(p, Some(yes), Some(no), None)?;
                    return Ok(());
                }
            } else if state == State::Completed {
                self.async_generator_settle(id, Value::Undefined, false, true)?;
                continue;
            }
            let Some(activation) = generator.activation else {
                self.async_generator_settle(id, Value::Undefined, false, true)?;
                continue;
            };
            self.set_generator_state(id, State::Executing)?;
            self.resume_program(activation, request.value, request.mode)?;
        }
    }
    pub(super) fn generator_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let mode = match name.rsplit('.').next().unwrap() {
            "next" => Resume::Next,
            "return" => Resume::Return,
            _ => Resume::Throw,
        };
        self.generator_resume(
            receiver,
            args.first().cloned().unwrap_or(Value::Undefined),
            mode,
        )
    }
}
