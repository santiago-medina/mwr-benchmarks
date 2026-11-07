//! Typed host completions; transport and origin policy belong to the embedder.
use super::{
    objects::{Descriptor, Key},
    promises::Capability,
    vm::{Engine, ScriptError, Value},
};
use crate::dual::{ExternalRoot, Id};
use std::collections::HashMap;
type Result<T> = std::result::Result<T, ScriptError>;

/// Headers only. The host retains the native response until release_responses.
#[derive(Clone, Debug)]
pub struct FetchResponse {
    pub handle: u64,
    pub status: u16,
    pub has_body: bool,
    pub url: String,
}
#[derive(Clone)]
pub(super) struct Response {
    pub headers: FetchResponse,
    pub used: bool,
}
#[derive(Clone)]
pub(super) struct Request {
    cap: Capability,
    response: Option<Value>,
}
impl Request {
    pub fn edges(&self) -> Vec<Id> {
        self.cap
            .values()
            .iter()
            .filter_map(Value::id)
            .chain(self.response.as_ref().and_then(Value::id))
            .collect()
    }
}
pub(super) struct Network {
    canceled: Vec<u64>,
    next: u64,
    pending: HashMap<u64, ExternalRoot>,
    resources: HashMap<Id, u64>,
    releases: Vec<u64>,
    prototype: Option<Id>,
}
impl Default for Network {
    fn default() -> Self {
        Self {
            canceled: vec![],
            next: 1,
            pending: HashMap::new(),
            resources: HashMap::new(),
            releases: vec![],
            prototype: None,
        }
    }
}
impl Engine<'_> {
    pub fn pending_fetches(&self) -> usize {
        self.network.pending.len()
    }
    pub(super) fn cancel_fetches(&mut self) {
        for (serial, root) in self.network.pending.drain() {
            self.network.canceled.push(serial);
            self.runtime.unroot(root).expect("owned fetch root");
        }
        self.flush_network();
    }
    pub(super) fn prune_responses(&mut self) {
        self.network.resources.retain(|id, handle| {
            let alive = self.runtime.value(*id).is_ok();
            if !alive {
                self.network.releases.push(*handle);
            }
            alive
        });
        if self
            .network
            .prototype
            .is_some_and(|id| self.runtime.value(id).is_err())
        {
            self.network.prototype = None;
        }
        self.flush_responses();
    }
    pub(super) fn flush_network(&mut self) {
        if let Some(host) = self.adapter.as_mut() {
            for serial in self.network.canceled.drain(..) {
                host.cancel_fetch(serial);
            }
        }
        self.flush_responses();
    }
    fn flush_responses(&mut self) {
        if self.network.releases.is_empty() {
            return;
        }
        if let Some(host) = self.adapter.as_mut() {
            match host.release_responses(&self.network.releases) {
                Ok(()) => self.network.releases.clear(),
                Err(e) => self.adapter_error = Some(e),
            }
        }
    }
    fn response_prototype(&mut self) -> Result<Id> {
        if let Some(id) = self.network.prototype {
            return Ok(id);
        }
        let env = self
            .runtime
            .resolve(self.global.ok_or(ScriptError::Runtime("no fetch realm"))?)?;
        let proto = self.new_object(Some(self.intrinsics.as_ref().unwrap().object))?;
        self.runtime.link(env, proto)?;
        self.network.prototype = Some(proto);
        for field in ["status", "ok", "url", "bodyUsed", "text"] {
            let function = self.intrinsic_function(
                env,
                &format!("Response.{field}"),
                Some(self.intrinsics.as_ref().unwrap().function),
            )?;
            let desc = if field == "text" {
                Descriptor {
                    value: Some(Value::Object(function)),
                    writable: Some(true),
                    enumerable: Some(true),
                    ..Descriptor::default()
                }
            } else {
                Descriptor {
                    get: Some(Value::Object(function)),
                    enumerable: Some(true),
                    ..Descriptor::default()
                }
            };
            self.define(
                proto,
                field,
                Descriptor {
                    configurable: Some(true),
                    ..desc
                },
            )?;
        }
        self.define(
            proto,
            Key::Symbol(1),
            Descriptor {
                value: Some(Value::String("Response".into())),
                configurable: Some(true),
                ..Descriptor::default()
            },
        )?;
        Ok(proto)
    }
    fn wrap_response(&mut self, headers: FetchResponse) -> Result<Value> {
        let proto = self.response_prototype()?;
        let id = self.new_object(Some(proto))?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.response = Some(Response {
            headers: headers.clone(),
            used: false,
        });
        self.replace_data(id, data)?;
        self.network.resources.insert(id, headers.handle);
        Ok(Value::Object(id))
    }
    fn request(&mut self, cap: Capability, response: Option<Value>) -> Result<u64> {
        if self.network.pending.len() >= 1024 {
            return Err(ScriptError::Runtime("pending fetch limit"));
        }
        let serial = self.network.next;
        self.network.next = serial
            .checked_add(1)
            .ok_or(ScriptError::Runtime("fetch serial exhausted"))?;
        let id = self.new_object(None)?;
        self.protect(&Value::Object(id))?;
        let mut data = self.data(id)?.clone();
        data.request = Some(Request { cap, response });
        self.replace_data(id, data)?;
        let root = self.runtime.root(id)?;
        self.network.pending.insert(serial, root);
        Ok(serial)
    }
    fn pending_request(&self, serial: u64, body: bool) -> Result<Option<Id>> {
        let Some(root) = self.network.pending.get(&serial) else {
            return Ok(None);
        };
        let id = self.runtime.resolve(*root)?;
        let request = self
            .data(id)?
            .request
            .as_ref()
            .ok_or(ScriptError::Runtime("missing fetch request"))?;
        if request.response.is_some() != body {
            return Err(ScriptError::Runtime("wrong fetch completion kind"));
        }
        Ok(Some(id))
    }
    fn settle_request(&mut self, serial: u64, value: Value, reject: bool) -> Result<()> {
        self.protect(&value)?;
        let root = self
            .network
            .pending
            .remove(&serial)
            .ok_or(ScriptError::Runtime("missing fetch serial"))?;
        let id = self.runtime.resolve(root)?;
        self.protect(&Value::Object(id))?;
        self.runtime.unroot(root)?;
        let request = self.data(id)?.request.clone().unwrap();
        let mut data = self.data(id)?.clone();
        data.request = None;
        self.replace_data(id, data)?;
        self.invoke(
            if reject {
                request.cap.reject
            } else {
                request.cap.resolve
            },
            vec![value],
        )?;
        Ok(())
    }
    /// Invoke between scripts, with the same host registry reattached. Never inline.
    pub fn complete_fetch(
        &mut self,
        serial: u64,
        result: std::result::Result<FetchResponse, String>,
    ) -> Result<bool> {
        self.complete_network(serial, false, |e| match result {
            Ok(headers) => {
                let handle = headers.handle;
                match e.wrap_response(headers) {
                    Ok(v) => Ok((v, false)),
                    Err(err) => {
                        e.network.releases.push(handle);
                        Err(err)
                    }
                }
            }
            Err(message) => Ok((e.error_value("TypeError", &message)?, true)),
        })
    }
    pub fn complete_response_text(
        &mut self,
        serial: u64,
        result: std::result::Result<String, String>,
    ) -> Result<bool> {
        self.complete_network(serial, true, |e| match result {
            Ok(body) => Ok((Value::String(body), false)),
            Err(message) => Ok((e.error_value("TypeError", &message)?, true)),
        })
    }
    fn complete_network(
        &mut self,
        serial: u64,
        body: bool,
        value: impl FnOnce(&mut Self) -> Result<(Value, bool)>,
    ) -> Result<bool> {
        if self.call_depth != 0 || self.native_depth != 0 || self.checkpoint_running {
            return Err(ScriptError::Runtime("reentrant fetch completion"));
        }
        if self.pending_request(serial, body)?.is_none() {
            return Ok(false);
        }
        self.steps_for_external_event();
        let result = self.frame(|e| {
            let (v, reject) = value(e)?;
            e.settle_request(serial, v, reject)?;
            Ok(true)
        });
        if result.as_ref().is_err_and(ScriptError::is_terminal) {
            self.cancel_microtasks();
        }
        self.collect();
        result
    }
    pub(super) fn fetch_builtin(
        &mut self,
        name: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if name != "fetch" && name != "Response.text" {
            let response = if let Value::Object(id) = &receiver {
                self.data(*id).ok().and_then(|d| d.response.clone())
            } else {
                None
            };
            let Some(response) = response else {
                return self.throw_error("TypeError", "incompatible Response receiver");
            };
            return match name {
                "Response.status" => Ok(Value::number(response.headers.status as f64)),
                "Response.ok" => Ok(Value::Bool((200..300).contains(&response.headers.status))),
                "Response.url" => Ok(Value::String(response.headers.url)),
                "Response.bodyUsed" => Ok(Value::Bool(response.used)),
                _ => Err(ScriptError::Unsupported("Response operation")),
            };
        }
        let cap = self.capability(Value::Object(
            self.intrinsics.as_ref().unwrap().functions["Promise"],
        ))?;
        if name == "fetch" {
            if !self.adapter.as_ref().is_some_and(|h| h.supports_fetch()) {
                return Err(ScriptError::Unsupported(
                    "fetch requires a native network adapter",
                ));
            }
            if args.is_empty() {
                let error = self.error_value("TypeError", "fetch requires a URL")?;
                self.invoke(cap.reject, vec![error])?;
                return Ok(cap.promise);
            }
            if args.get(1).is_some_and(|v| *v != Value::Undefined) {
                return Err(ScriptError::Unsupported("fetch RequestInit"));
            }
            let url = match self.to_string(args.first().cloned().unwrap_or(Value::Undefined)) {
                Ok(url) => url,
                Err(ScriptError::Thrown(ex)) => {
                    self.invoke(cap.reject, vec![ex.value])?;
                    return Ok(cap.promise);
                }
                Err(e) => return Err(e),
            };
            let serial = self.request(cap.clone(), None)?;
            if let Err(message) = self.adapter.as_mut().unwrap().start_fetch(serial, &url) {
                let error = self.error_value("TypeError", &message)?;
                self.settle_request(serial, error, true)?;
            }
        } else {
            let response = if let Value::Object(id) = receiver {
                self.data(id)
                    .ok()
                    .and_then(|d| d.response.clone())
                    .map(|r| (id, r))
            } else {
                None
            };
            let Some((id, response)) = response else {
                let error = self.error_value("TypeError", "incompatible Response receiver")?;
                self.invoke(cap.reject, vec![error])?;
                return Ok(cap.promise);
            };
            if response.used {
                let error = self.error_value("TypeError", "response body already consumed")?;
                self.invoke(cap.reject, vec![error])?;
                return Ok(cap.promise);
            }
            if !response.headers.has_body {
                self.invoke(cap.resolve, vec![Value::String(String::new())])?;
                return Ok(cap.promise);
            }
            let serial = self.request(cap.clone(), Some(receiver))?;
            let mut data = self.data(id)?.clone();
            data.response.as_mut().unwrap().used = response.headers.has_body;
            self.replace_data(id, data)?;
            if let Err(message) = self
                .adapter
                .as_mut()
                .ok_or(ScriptError::Runtime("response host detached"))?
                .read_response_text(serial, response.headers.handle)
            {
                let error = self.error_value("TypeError", &message)?;
                self.settle_request(serial, error, true)?;
            }
        }
        Ok(cap.promise)
    }
}
