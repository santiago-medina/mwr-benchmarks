//! Mock DOM by default; an optional typed adapter connects real foreign nodes.
//! Source and closure bodies stay in the interpreter, never in the adapter.
use super::objects::Key;
use super::vm::{Engine, Object, ScriptError, Value};
use crate::dual::{Domain, ExternalRoot, Id};
type Result<T> = std::result::Result<T, ScriptError>;

impl Engine<'_> {
    pub fn text_content(&mut self, binding: &str) -> Result<String> {
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
    pub fn retained_text(&mut self, root: ExternalRoot) -> Result<String> {
        self.node_text(self.runtime.resolve(root)?)
    }
    pub fn release_node(&mut self, root: ExternalRoot) -> Result<()> {
        self.runtime.unroot(root)?;
        self.collect();
        Ok(())
    }
    pub fn remote_node_count(&self) -> usize {
        self.remote_nodes.len() + self.pending_release.len()
    }
    fn node_text(&mut self, id: Id) -> Result<String> {
        self.runtime.value(id)?;
        if let Some(remote) = self.remote_nodes.get(&id).copied() {
            return self
                .adapter
                .as_mut()
                .unwrap()
                .text_content(remote)
                .map_err(ScriptError::Host);
        }
        match self.objects.get(&id) {
            Some(Object::Node { text, .. }) => Ok(text.clone()),
            _ => Err(ScriptError::Runtime("not a node")),
        }
    }
    fn wrap_node(&mut self, remote: Option<u64>, tag: String) -> Result<Value> {
        if let Some(remote) = remote {
            if let Some((&id, _)) = self.remote_nodes.iter().find(|(_, r)| **r == remote) {
                return Ok(Value::Object(id));
            }
        }
        let id = match self.allocate(Domain::Dom, -100) {
            Ok(id) => id,
            Err(error) => {
                if let Some(remote) = remote {
                    self.queue_remote_release(remote);
                }
                return Err(error);
            }
        };
        self.objects.insert(
            id,
            Object::Node {
                tag,
                text: String::new(),
                listeners: vec![],
            },
        );
        if let Some(remote) = remote {
            self.remote_nodes.insert(id, remote);
        }
        Ok(Value::Object(id))
    }
    pub(super) fn get_member(
        &mut self,
        receiver: Value,
        property: impl Into<Key>,
    ) -> Result<Value> {
        let property = property.into();
        if let Key::Symbol(_) = property {
            return if let Value::Object(id) = receiver.clone() {
                self.ordinary_get(id, &property, receiver)
            } else {
                self.throw_error("TypeError", "symbol property on primitive not implemented")
            };
        }
        let Key::String(property) = property else {
            unreachable!()
        };
        match self.get_member_inner(receiver, &property) {
            Err(ScriptError::Host(s)) if self.adapter_error.is_none() => {
                self.throw_error("Error", &s)
            }
            r => r,
        }
    }
    fn get_member_inner(&mut self, receiver: Value, property: &str) -> Result<Value> {
        match receiver {
            Value::Document if matches!(property, "createElement" | "querySelector") => {
                Ok(Value::Native {
                    receiver: None,
                    name: property.into(),
                })
            }
            Value::Object(id) => {
                self.runtime.value(id)?;
                let Some(Object::Node { tag, .. }) = self.objects.get(&id) else {
                    return self.ordinary_get(id, property, Value::Object(id));
                };
                match property {
                    "textContent" => Ok(Value::String(self.node_text(id)?)),
                    "tagName" => Ok(Value::String(tag.to_ascii_uppercase())),
                    "addEventListener"
                    | "removeEventListener"
                    | "click"
                    | "appendChild"
                    | "removeChild" => Ok(Value::Native {
                        receiver: Some(id),
                        name: property.into(),
                    }),
                    _ => Err(ScriptError::Runtime("unsupported node property")),
                }
            }
            Value::Null | Value::Undefined => self.throw_error("TypeError", "null or undefined"),
            value => {
                let id = self.box_value(value.clone())?;
                self.protect(&Value::Object(id))?;
                self.ordinary_get(id, property, value)
            }
        }
    }
    pub(super) fn set_member(
        &mut self,
        receiver: Value,
        property: impl Into<Key>,
        value: Value,
    ) -> Result<()> {
        let property = property.into();
        if let Key::Symbol(_) = property {
            return if let Value::Object(id) = receiver.clone() {
                if self.ordinary_set(id, &property, value, receiver)? {
                    Ok(())
                } else {
                    self.throw_error("TypeError", "assignment refused")
                }
            } else {
                self.throw_error("TypeError", "invalid property receiver")
            };
        }
        let Key::String(property) = property else {
            unreachable!()
        };
        match self.set_member_inner(receiver, &property, value) {
            Err(ScriptError::Host(s)) if self.adapter_error.is_none() => {
                self.throw_error("Error", &s)
            }
            r => r,
        }
    }
    fn set_member_inner(&mut self, receiver: Value, property: &str, value: Value) -> Result<()> {
        let Value::Object(id) = receiver else {
            return Err(ScriptError::Runtime("not a node"));
        };
        self.runtime.value(id)?;
        if !matches!(self.objects.get(&id), Some(Object::Node { .. })) {
            return if self.ordinary_set(id, property, value, Value::Object(id))? {
                Ok(())
            } else {
                self.throw_error("TypeError", "assignment refused")
            };
        }
        if property != "textContent" {
            return Err(ScriptError::Unsupported("host property assignment"));
        }
        let value = if value == Value::Null {
            String::new()
        } else {
            self.to_string(value)?
        };
        if let Some(remote) = self.remote_nodes.get(&id).copied() {
            self.adapter
                .as_mut()
                .unwrap()
                .set_text_content(remote, &value)
                .map_err(ScriptError::Host)?;
        }
        let Some(Object::Node { text, .. }) = self.objects.get_mut(&id) else {
            unreachable!()
        };
        *text = value;
        Ok(())
    }
    /// Deliver a native event already received by the browser; do not synthesize
    /// another DOM click. Unknown/released foreign nodes are rejected.
    pub fn dispatch_remote_click(&mut self, remote: u64) -> Result<()> {
        // Only a new external invocation gets a budget. Programmatic/reentrant
        // delivery belongs to its caller and cannot reset that caller's limit.
        if self.call_depth == 0 && self.native_depth == 0 {
            self.steps_for_external_event();
        }
        let id = self
            .remote_nodes
            .iter()
            .find(|(_, r)| **r == remote)
            .map(|(id, _)| *id)
            .ok_or(ScriptError::Runtime("event for released node"))?;
        self.dispatch_node(id).map(|_| ())
    }
    fn dispatch_node(&mut self, node: Id) -> Result<Value> {
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
                    e.invoke_this(Value::Object(callback), Value::Object(node), vec![])?;
                }
            }
            Ok(Value::Undefined)
        });
        self.native_depth -= 1;
        result
    }
    pub(super) fn call_native(
        &mut self,
        receiver: Option<Id>,
        name: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        if let Some(error) = self.adapter_error.clone() {
            return Err(ScriptError::Host(error));
        }
        match (receiver, name) {
            (None, "createElement" | "querySelector") => {
                let [Value::String(argument)] = args.as_slice() else {
                    return Err(ScriptError::Runtime("DOM lookup requires one string"));
                };
                if name == "querySelector" {
                    let adapter = self.adapter.as_mut().ok_or(ScriptError::Runtime(
                        "querySelector requires an external DOM",
                    ))?;
                    let remote = adapter
                        .query_selector(argument)
                        .map_err(ScriptError::Host)?;
                    if let Some((&id, _)) = self.remote_nodes.iter().find(|(_, r)| **r == remote) {
                        return Ok(Value::Object(id));
                    }
                    let tag = match adapter.tag_name(remote) {
                        Ok(tag) => tag,
                        Err(error) => {
                            self.queue_remote_release(remote);
                            return Err(ScriptError::Host(error));
                        }
                    };
                    self.wrap_node(Some(remote), tag)
                } else {
                    if argument.is_empty()
                        || argument.len() > 64
                        || !argument
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-')
                    {
                        return Err(ScriptError::Runtime("unsupported tag name"));
                    }
                    let remote = match self.adapter.as_mut() {
                        Some(a) => Some(a.create_element(argument).map_err(ScriptError::Host)?),
                        None => None,
                    };
                    self.wrap_node(remote, argument.clone())
                }
            }
            (Some(parent), "appendChild" | "removeChild") => {
                let [Value::Object(child)] = args.as_slice() else {
                    return self.throw_error("TypeError", "tree operation requires one node");
                };
                let (Some(parent), Some(child_id)) = (
                    self.remote_nodes.get(&parent).copied(),
                    self.remote_nodes.get(child).copied(),
                ) else {
                    return Err(ScriptError::Unsupported(
                        "tree operations require an external DOM adapter",
                    ));
                };
                let adapter = self.adapter.as_mut().unwrap();
                if name == "appendChild" {
                    adapter.append_child(parent, child_id)
                } else {
                    adapter.remove_child(parent, child_id)
                }
                .map_err(ScriptError::Host)?;
                Ok(Value::Object(*child))
            }
            (Some(node), "addEventListener" | "removeEventListener") => {
                let [Value::String(event), Value::Object(callback)] = args.as_slice() else {
                    return Err(ScriptError::Runtime("listener requires event and closure"));
                };
                if event != "click" || !self.callable(&Value::Object(*callback)) {
                    return Err(ScriptError::Runtime("only click closures are supported"));
                }
                self.runtime.value(node)?;
                let Some(Object::Node { listeners, .. }) = self.objects.get(&node) else {
                    return Err(ScriptError::Runtime("not a node"));
                };
                let present = listeners.contains(callback);
                let count = listeners.len();
                if name == "addEventListener" && !present {
                    self.runtime.link(node, *callback)?;
                    if count == 0 {
                        if let Some(remote) = self.remote_nodes.get(&node).copied() {
                            if let Err(error) = self
                                .adapter
                                .as_mut()
                                .unwrap()
                                .set_click_listener(remote, true)
                            {
                                self.runtime.unlink(node, *callback)?;
                                return Err(ScriptError::Host(error));
                            }
                        }
                    }
                    let Some(Object::Node { listeners, .. }) = self.objects.get_mut(&node) else {
                        unreachable!()
                    };
                    listeners.push(*callback);
                    if self
                        .adapter
                        .as_ref()
                        .is_some_and(|a| a.traces_native_edges())
                    {
                        self.runtime.pin_foreign(node, true)?;
                    }
                } else if name == "removeEventListener" && present {
                    if count == 1 {
                        if let Some(remote) = self.remote_nodes.get(&node).copied() {
                            self.adapter
                                .as_mut()
                                .unwrap()
                                .set_click_listener(remote, false)
                                .map_err(ScriptError::Host)?;
                        }
                    }
                    self.runtime.unlink(node, *callback)?;
                    let Some(Object::Node { listeners, .. }) = self.objects.get_mut(&node) else {
                        unreachable!()
                    };
                    listeners.retain(|id| id != callback);
                    if listeners.is_empty() {
                        self.runtime.pin_foreign(node, false)?;
                    }
                }
                Ok(Value::Undefined)
            }
            (Some(node), "click") => {
                if !args.is_empty() {
                    return Err(ScriptError::Runtime("click takes no arguments"));
                }
                if let Some(remote) = self.remote_nodes.get(&node).copied() {
                    let events = self
                        .adapter
                        .as_mut()
                        .unwrap()
                        .click(remote)
                        .map_err(ScriptError::Host)?;
                    for event in events {
                        self.dispatch_remote_click(event)?;
                    }
                    Ok(Value::Undefined)
                } else {
                    self.dispatch_node(node)
                }
            }
            _ => Err(ScriptError::Runtime("unknown native operation")),
        }
    }
}
