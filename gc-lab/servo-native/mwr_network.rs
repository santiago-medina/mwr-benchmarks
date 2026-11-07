//! Native Fetch/Response promises. Callbacks retain identifiers, never sessions.
use super::*;
use crate::dom::abortcontroller::AbortController;
use crate::dom::bindings::codegen::Bindings::RequestBinding::RequestInit;
use crate::dom::bindings::codegen::Bindings::ResponseBinding::ResponseMethods;
use crate::dom::bindings::codegen::Bindings::WindowBinding::WindowMethods;
use crate::dom::bindings::codegen::UnionTypes::RequestOrUSVString;
use crate::dom::bindings::conversions::root_from_object;
use crate::dom::bindings::str::USVString;
use crate::dom::promisenativehandler::{Callback, PromiseNativeHandler};
use crate::dom::response::Response;
use gc_lab::script::FetchResponse;
use js::{
    conversions::{ConversionResult, FromJSValConvertible},
    jsval::UndefinedValue,
    realm::CurrentRealm,
    rooted,
    rust::HandleValue,
};

#[derive(Default)]
pub(super) struct Network {
    pending: HashMap<u64, Trusted<AbortController>>,
    responses: HashMap<u64, (Trusted<Response>, Trusted<AbortController>)>,
}
#[derive(MallocSizeOf)]
struct Completion {
    pipeline: PipelineId,
    serial: u64,
    body: bool,
    reject: bool,
}
#[expect(unsafe_code)]
unsafe impl JSTraceable for Completion {
    // All managed objects are owned by native promises or the session registry.
    unsafe fn trace(&self, _: *mut JSTracer) {}
}
impl Callback for Completion {
    fn callback(&self, cx: &mut CurrentRealm, value: HandleValue) {
        if let Err(error) = self.deliver(cx, value) {
            eprintln!("MWR_NATIVE_ERROR fetch completion: {error}");
        }
    }
}
impl Completion {
    #[expect(unsafe_code)]
    fn deliver(&self, cx: &mut CurrentRealm, value: HandleValue) -> Result<()> {
        let Some(state) = SESSIONS.with(|s| s.borrow().get(&self.pipeline).cloned()) else {
            eprintln!("MWR_NATIVE_STALE_FETCH");
            return Ok(());
        };
        let Some(controller) = state
            .registry
            .borrow_mut()
            .network
            .pending
            .remove(&self.serial)
        else {
            eprintln!("MWR_NATIVE_STALE_FETCH");
            return Ok(());
        };
        let doc = state.registry.borrow().document()?;
        enum Input {
            Body(std::result::Result<String, String>),
            Headers(std::result::Result<FetchResponse, String>),
        }
        let input = if self.body {
            let body = if self.reject {
                Err("native response body failed".into())
            } else {
                match DOMString::from_jsval(cx, value, Default::default()) {
                    Ok(ConversionResult::Success(text)) => Ok(text.to_string()),
                    _ => Err("native response text conversion failed".into()),
                }
            };
            Input::Body(body)
        } else {
            let headers = if self.reject {
                Err("native fetch rejected (network, origin or policy)".into())
            } else if !value.is_object() {
                Err("native fetch did not return a Response".into())
            } else {
                match unsafe { root_from_object::<Response>(cx, value.to_object()) } {
                    Ok(response) => {
                        let result = FetchResponse {
                            handle: self.serial,
                            status: response.Status(),
                            url: response.Url().to_string(),
                        };
                        state
                            .registry
                            .borrow_mut()
                            .network
                            .responses
                            .insert(self.serial, (Trusted::new(&*response), controller));
                        Ok(result)
                    }
                    Err(_) => Err("native fetch result is not a Response".into()),
                }
            };
            Input::Headers(headers)
        };
        let dormant = state
            .engine
            .borrow_mut()
            .take()
            .ok_or("fetch completion during active VM")?;
        let mut engine = dormant.resume(Box::new(NativeAdapter::new(cx, state.registry.clone())));
        let result = match input {
            Input::Body(body) => engine.complete_response_text(self.serial, body),
            Input::Headers(headers) => engine.complete_fetch(self.serial, headers),
        };
        let host_error = engine.host_error().map(str::to_owned);
        *state.engine.borrow_mut() = Some(engine.suspend());
        result.map_err(|e| format!("{e:?}"))?;
        if let Some(error) = host_error {
            return Err(error);
        }
        eprintln!(
            "MWR_NATIVE_FETCH_COMPLETION {} {}",
            self.serial,
            if self.body { "body" } else { "headers" }
        );
        queue_collection(&doc);
        queue_check(&doc);
        Ok(())
    }
}
fn handler(
    cx: &mut JSContext,
    doc: &Document,
    serial: u64,
    body: bool,
) -> DomRoot<PromiseNativeHandler> {
    let pipeline = doc.window().pipeline_id();
    PromiseNativeHandler::new(
        cx,
        &doc.global(),
        Some(Box::new(Completion {
            pipeline,
            serial,
            body,
            reject: false,
        })),
        Some(Box::new(Completion {
            pipeline,
            serial,
            body,
            reject: true,
        })),
    )
}
fn abort(cx: &mut JSContext, controller: Trusted<AbortController>) {
    let controller = controller.root();
    let global = controller.global();
    let mut realm = crate::realms::enter_auto_realm(cx, &*global);
    let mut cx = realm.current_realm();
    rooted!(&in(&mut cx) let reason=UndefinedValue());
    controller.signal_abort(&mut cx, reason.handle());
}
impl NativeAdapter<'_> {
    pub(super) fn start_network(&mut self, serial: u64, url: &str) -> Result<()> {
        let doc = self.registry.borrow().document()?;
        let global = doc.global();
        let mut realm = crate::realms::enter_auto_realm(self.cx, &*global);
        let mut cx = realm.current_realm();
        let controller = AbortController::new_with_proto(&mut cx, &global, None);
        let mut init = RequestInit::empty();
        init.signal = Some(Some(controller.signal()));
        let promise = doc.window().Fetch(
            &mut cx,
            RequestOrUSVString::USVString(USVString(url.into())),
            &init,
        );
        self.registry
            .borrow_mut()
            .network
            .pending
            .insert(serial, Trusted::new(&*controller));
        let callback = handler(&mut cx, &doc, serial, false);
        promise.append_native_handler(&mut cx, &callback);
        Ok(())
    }
    pub(super) fn read_network_text(&mut self, serial: u64, handle: u64) -> Result<()> {
        let doc = self.registry.borrow().document()?;
        let (response, controller) = {
            let registry = self.registry.borrow();
            let (r, c) = registry
                .network
                .responses
                .get(&handle)
                .ok_or("released native response")?;
            (r.root(), Trusted::new(&*c.root()))
        };
        let promise = response.Text(self.cx);
        self.registry
            .borrow_mut()
            .network
            .pending
            .insert(serial, controller);
        let callback = handler(self.cx, &doc, serial, true);
        promise.append_native_handler(self.cx, &callback);
        Ok(())
    }
    pub(super) fn cancel_network(&mut self, serial: u64) {
        let controller = self.registry.borrow_mut().network.pending.remove(&serial);
        if let Some(controller) = controller {
            abort(self.cx, controller);
        }
    }
    pub(super) fn release_network(&mut self, handles: &[u64]) {
        let controllers: Vec<_> = {
            let mut registry = self.registry.borrow_mut();
            handles
                .iter()
                .filter_map(|id| registry.network.responses.remove(id).map(|(_, c)| c))
                .collect()
        };
        for controller in controllers {
            abort(self.cx, controller);
        }
    }
}

// Observe after the native Promise callback and VM microtask checkpoint finish.
fn queue_check(doc: &Document) {
    if std::env::var_os("MWR_NATIVE_FETCH_TEST").is_none() {
        return;
    }
    let document = Trusted::new(doc);
    doc.global().task_manager().dom_manipulation_task_source().queue(task!(MwrFetchCheck: move |cx| {
        let doc=document.root();
        let expected=std::env::var("MWR_NATIVE_FETCH_EXPECT").unwrap_or_default();
        let result=doc.QuerySelector(cx,DOMString::from("#result")).ok().flatten();
        if let Some(result)=result {
            let text=result.upcast::<Node>().GetTextContent().map(|s|s.to_string()).unwrap_or_default();
            if text==expected { eprintln!("MWR_NATIVE_FETCH_PROOF {}",serde_json::json!({"after":text,"native_fetch":true})); }
        }
    }));
}
