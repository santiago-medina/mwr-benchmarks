//! Minimal synchronous DOM adapter. Nodes belong to the DOM arena; listeners
//! are parsed JS closures in the other arena. All ownership edges are traced.
use super::vm::{Engine, Object, ScriptError, Value};
use crate::dual::{Domain, ExternalRoot, Id};
type Result<T> = std::result::Result<T, ScriptError>;

impl Engine {
    pub fn text_content(&self, binding: &str) -> Result<String> {
        let Value::Object(id) = self.binding(binding)? else {
            return Err(ScriptError::Runtime("not a node"));
        };
        self.node_text(id)
    }
    pub fn retain_node(&mut self, binding: &str) -> Result<ExternalRoot> {
        let Value::Object(id) = self.binding(binding)? else {
            return Err(ScriptError::Runtime("not a node"));
        };
        self.node_text(id)?;
        Ok(self.runtime.root(id)?)
    }
    pub fn retained_text(&self, root: ExternalRoot) -> Result<String> {
        self.node_text(self.runtime.resolve(root)?)
    }
    pub fn release_node(&mut self, root: ExternalRoot) -> Result<()> {
        self.runtime.unroot(root)?;
        self.collect();
        Ok(())
    }
    fn node_text(&self, id: Id) -> Result<String> {
        self.runtime.value(id)?;
        match self.objects.get(&id) {
            Some(Object::Node { text, .. }) => Ok(text.clone()),
            _ => Err(ScriptError::Runtime("not a node")),
        }
    }
    pub(super) fn get_member(&mut self, receiver: Value, property: &str) -> Result<Value> {
        match receiver {
            Value::Document if property == "createElement" => Ok(Value::Native {
                receiver: None,
                name: property.into(),
            }),
            Value::Object(id) => {
                self.runtime.value(id)?;
                let Some(Object::Node { text, tag, .. }) = self.objects.get(&id) else {
                    return Err(ScriptError::Runtime("unsupported object property"));
                };
                match property {
                    "textContent" => Ok(Value::String(text.clone())),
                    "tagName" => Ok(Value::String(tag.to_ascii_uppercase())),
                    "addEventListener" | "removeEventListener" | "click" => Ok(Value::Native {
                        receiver: Some(id),
                        name: property.into(),
                    }),
                    _ => Err(ScriptError::Runtime("unsupported node property")),
                }
            }
            _ => Err(ScriptError::Runtime("unsupported property receiver")),
        }
    }
    pub(super) fn set_member(
        &mut self,
        receiver: Value,
        property: &str,
        value: Value,
    ) -> Result<()> {
        let Value::Object(id) = receiver else {
            return Err(ScriptError::Runtime("not a node"));
        };
        let Value::String(value) = value else {
            return Err(ScriptError::Runtime(
                "textContent requires a string in this subset",
            ));
        };
        self.runtime.value(id)?;
        let Some(Object::Node { text, .. }) = self.objects.get_mut(&id) else {
            return Err(ScriptError::Runtime("not a node"));
        };
        if property != "textContent" {
            return Err(ScriptError::Runtime("unsupported assignment"));
        }
        *text = value;
        Ok(())
    }
    pub(super) fn call_native(
        &mut self,
        receiver: Option<Id>,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        match (receiver, name) {
            (None, "createElement") => {
                let [Value::String(tag)] = args.as_slice() else {
                    return Err(ScriptError::Runtime("createElement requires one string"));
                };
                if tag.is_empty()
                    || tag.len() > 64
                    || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    return Err(ScriptError::Runtime("unsupported tag name"));
                }
                let id = self.allocate(Domain::Dom, -100)?;
                self.objects.insert(
                    id,
                    Object::Node {
                        tag: tag.clone(),
                        text: String::new(),
                        listeners: vec![],
                    },
                );
                Ok(Value::Object(id))
            }
            (Some(node), "addEventListener" | "removeEventListener") => {
                let [Value::String(event), Value::Object(callback)] = args.as_slice() else {
                    return Err(ScriptError::Runtime("listener requires event and closure"));
                };
                if event != "click"
                    || !matches!(self.objects.get(callback), Some(Object::Closure { .. }))
                {
                    return Err(ScriptError::Runtime("only click closures are supported"));
                }
                self.runtime.value(node)?;
                let Some(Object::Node { listeners, .. }) = self.objects.get(&node) else {
                    return Err(ScriptError::Runtime("not a node"));
                };
                let present = listeners.contains(callback);
                if name == "addEventListener" && !present {
                    self.runtime.link(node, *callback)?;
                    let Some(Object::Node { listeners, .. }) = self.objects.get_mut(&node) else {
                        unreachable!()
                    };
                    listeners.push(*callback);
                } else if name == "removeEventListener" && present {
                    self.runtime.unlink(node, *callback)?;
                    let Some(Object::Node { listeners, .. }) = self.objects.get_mut(&node) else {
                        unreachable!()
                    };
                    listeners.retain(|id| id != callback);
                }
                Ok(Value::Undefined)
            }
            (Some(node), "click") => {
                if !args.is_empty() {
                    return Err(ScriptError::Runtime("click takes no arguments"));
                }
                self.runtime.value(node)?;
                let Some(Object::Node { listeners, .. }) = self.objects.get(&node) else {
                    return Err(ScriptError::Runtime("not a node"));
                };
                let snapshot = listeners.clone();
                if self.native_depth >= 32 {
                    return Err(ScriptError::Runtime("native reentry limit"));
                }
                self.native_depth += 1;
                let result = self.frame(|e| {
                    e.protect(&Value::Object(node))?;
                    for callback in &snapshot {
                        e.protect(&Value::Object(*callback))?;
                    }
                    for callback in snapshot {
                        let Some(Object::Node { listeners, .. }) = e.objects.get(&node) else {
                            unreachable!()
                        };
                        if listeners.contains(&callback) {
                            e.invoke(Value::Object(callback), vec![])?;
                        }
                    }
                    Ok(Value::Undefined)
                });
                self.native_depth -= 1;
                result
            }
            _ => Err(ScriptError::Runtime("unknown native operation")),
        }
    }
}
