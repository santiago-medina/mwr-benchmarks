//! Generator state and suspended frames are traced records; resumption shares the async VM.
use super::{
    asynchronous::{Program, Target},
    objects::{Descriptor, Key},
    parser::{Param, Stmt},
    vm::{Engine, FunctionKind, ScriptError, Value},
};
use crate::dual::Id;
use std::{collections::HashMap, sync::Arc};
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Start,
    Yield,
    Executing,
    Completed,
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
}
impl Generator {
    pub fn edges(&self) -> Vec<Id> {
        let mut ids: Vec<_> = self.activation.into_iter().collect();
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
        let local = self.function_environment(env, &params, &body, kind, receiver, &args)?;
        let prototype = self.get_member(Value::Object(function), "prototype")?;
        let prototype = if let Value::Object(id) = prototype {
            id
        } else {
            self.intrinsics.as_ref().unwrap().collection["Generator"]
        };
        let id = self.new_object(Some(prototype))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.generator = Some(Generator {
            state: State::Start,
            activation: None,
            output: None,
        });
        self.replace_data(id, data)?;
        let activation = self.new_activation(local, program, Target::Generator(id))?;
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
        let Some(generator) = self.data(id).ok().and_then(|d| d.generator.clone()) else {
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
        Ok(())
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
