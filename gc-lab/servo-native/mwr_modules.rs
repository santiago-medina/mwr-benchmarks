//! Script-destination, CORS module downloads. No source is compiled by SpiderMonkey.
use super::*;
use crate::dom::bindings::codegen::Bindings::DocumentBinding::DocumentReadyState;
use crate::dom::csp::{GlobalCspReporting, Violation};
use crate::dom::globalscope::GlobalScope;
use crate::event_loop::document_loader::LoadType;
use crate::fetch::network_listener::{FetchResponseListener, NetworkListener};
use crate::modules::script_module::ScriptFetchOptions;
use bytes::Bytes;
use encoding_rs::UTF_8;
use gc_lab::script::{parser::parse_module, ModuleSource, ModuleStatus};
use headers::{HeaderMapExt, ReferrerPolicy as ReferrerPolicyHeader};
use net_traits::mime_classifier::MimeClassifier;
use net_traits::request::{Destination, RequestBuilder, RequestId, RequestMode};
use net_traits::{FetchMetadata, Metadata, NetworkError, ResourceFetchTiming};
use servo_url::ServoUrl;
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct Load {
    entry: String,
    blocker: Option<ServoUrl>,
    sources: HashMap<String, ModuleSource>,
    pending: HashMap<String, Lease>,
    failed: Option<String>,
    started: bool,
    reported: bool,
    waiting: bool,
    bytes: usize,
}
struct Lease {
    context: Option<Arc<Mutex<Option<SourceListener>>>>,
    global: Arc<Mutex<Option<Trusted<GlobalScope>>>>,
    id: RequestId,
}
impl Lease {
    fn finish(mut self) {
        self.context = None;
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(context) = &self.context {
            context.lock().unwrap().take();
        }
        self.global.lock().unwrap().take();
    }
}
struct SourceListener {
    pipeline: PipelineId,
    url: String,
    options: ScriptFetchOptions,
    data: Vec<u8>,
    metadata: Option<Metadata>,
    error: Option<String>,
}
fn resolve(base: &ServoUrl, specifier: &str) -> Result<ServoUrl> {
    if let Ok(url) = ServoUrl::parse(specifier) {
        return Ok(url);
    }
    if !specifier.starts_with(['.', '/']) {
        return Err("bare module specifiers/import maps are unsupported".into());
    }
    base.join(specifier).map_err(|e| e.to_string())
}
pub(super) fn begin(
    cx: &mut JSContext,
    doc: &Document,
    state: &Rc<Session>,
    text: &str,
    src: Option<String>,
    options: ScriptFetchOptions,
) -> Result<()> {
    let mut blocker = doc.url();
    blocker.set_fragment(Some("mwr-module-loader"));
    doc.loader_mut()
        .add_blocking_load(LoadType::Script(blocker.clone()));
    {
        let mut load = state.module_load.borrow_mut();
        load.entry = blocker.to_string();
        load.blocker = Some(blocker);
    }
    if doc
        .QuerySelector(cx, DOMString::from("script[type='importmap']"))
        .map_err(|e| format!("{e:?}"))?
        .is_some()
    {
        return Err("import maps are unsupported".into());
    }
    let base = doc.base_url();
    if let Some(src) = src {
        let entry = base.join(&src).map_err(|e| e.to_string())?;
        state.module_load.borrow_mut().entry = entry.to_string();
        request(cx, doc, state, entry, options)?;
    } else {
        let mut entry = base.clone();
        entry.set_fragment(Some("mwr-inline-module"));
        state.module_load.borrow_mut().entry = entry.to_string();
        add_source(
            cx,
            doc,
            state,
            entry.to_string(),
            base,
            text.to_string(),
            options,
        )?;
    }
    Ok(())
}
fn request(
    cx: &mut JSContext,
    doc: &Document,
    state: &Rc<Session>,
    url: ServoUrl,
    options: ScriptFetchOptions,
) -> Result<()> {
    let key = url.to_string();
    {
        let load = state.module_load.borrow();
        if load.sources.contains_key(&key) || load.pending.contains_key(&key) {
            return Ok(());
        }
        if load.sources.len() + load.pending.len() >= 128 {
            return Err("module graph limit".into());
        }
    }
    let global = doc.global();
    let request = RequestBuilder::new(
        global.webview_id(),
        crate::url::ensure_blob_referenced_by_url_is_kept_alive(&global, url),
        global.get_referrer(),
    )
    .destination(Destination::Script)
    .mode(RequestMode::CorsMode)
    .parser_metadata(options.parser_metadata)
    .integrity_metadata(options.integrity_metadata.clone())
    .credentials_mode(options.credentials_mode)
    .referrer_policy(options.referrer_policy)
    .cryptographic_nonce_metadata(options.cryptographic_nonce.clone())
    .client(global.request_client(Some(cx.no_gc())))
    .pipeline_id(Some(global.pipeline_id()));
    let id = request.id;
    let listener = NetworkListener::new(
        SourceListener {
            pipeline: doc.window().pipeline_id(),
            url: key.clone(),
            options,
            data: vec![],
            metadata: None,
            error: None,
        },
        global.task_manager().networking_task_source().to_sendable(),
        &global,
    );
    state.module_load.borrow_mut().pending.insert(
        key.clone(),
        Lease {
            context: Some(listener.context.clone()),
            global: listener.global_scope.clone(),
            id,
        },
    );
    global.fetch_group_mut().fetch(request, listener);
    eprintln!("MWR_NATIVE_MODULE_REQUEST {key}");
    Ok(())
}
fn add_source(
    cx: &mut JSContext,
    doc: &Document,
    state: &Rc<Session>,
    url: String,
    base: ServoUrl,
    source: String,
    options: ScriptFetchOptions,
) -> Result<()> {
    let syntax = parse_module(&source).map_err(|e| format!("module syntax: {e:?}"))?;
    let mut resolutions = vec![];
    let mut requests = vec![];
    for specifier in syntax.dependencies() {
        let target = resolve(&base, specifier)?;
        resolutions.push((specifier.to_string(), target.to_string()));
        requests.push(target);
    }
    {
        let mut load = state.module_load.borrow_mut();
        load.bytes += source.len();
        if load.bytes > 1048576 {
            return Err("module source budget".into());
        }
        load.sources.insert(
            url.clone(),
            ModuleSource {
                url,
                source,
                resolutions,
            },
        );
    }
    for url in requests {
        let child_options = options.descendant_fetch_options(&url, &doc.global());
        request(cx, doc, state, url, child_options)?;
    }
    queue_start(doc, state);
    Ok(())
}
impl FetchResponseListener for SourceListener {
    fn process_request_body(&mut self, _: RequestId) {}
    fn process_response(
        &mut self,
        _: &mut JSContext,
        _: RequestId,
        result: std::result::Result<FetchMetadata, NetworkError>,
    ) {
        match result {
            Ok(metadata) => {
                let metadata = match metadata {
                    FetchMetadata::Unfiltered(m) => m,
                    FetchMetadata::Filtered { unsafe_, .. } => unsafe_,
                };
                if !metadata.status.is_success() {
                    self.error = Some("module HTTP status is not successful".into());
                }
                self.metadata = Some(metadata);
                eprintln!("MWR_NATIVE_MODULE_HEADERS {}", self.url);
                if std::env::var_os("MWR_NATIVE_MODULE_NAVIGATION").is_some()
                    && self.url.ends_with("/slow.js")
                {
                    if let Some(state) = SESSIONS.with(|s| s.borrow().get(&self.pipeline).cloned())
                    {
                        if let Ok(doc) = state.registry.borrow().document() {
                            let document = Trusted::new(&*doc);
                            doc.global().task_manager().dom_manipulation_task_source().queue(task!(MwrPendingModuleNavigation:move|cx|{
                                let doc=document.root();let mut realm=crate::realms::enter_auto_realm(cx,doc.window());
                                if let Err(error)=network::begin_module_navigation(&mut realm.current_realm(),&doc){eprintln!("MWR_NATIVE_ERROR module navigation: {error}");}
                            }));
                        }
                    }
                }
            }
            Err(error) => self.error = Some(format!("module network/policy error: {error:?}")),
        }
    }
    fn process_response_chunk(&mut self, _: &mut JSContext, _: RequestId, data: Bytes) {
        if self.data.len() + data.len() > 65536 {
            self.error = Some("module source limit".into());
        }
        if self.error.is_none() {
            self.data.extend_from_slice(&data);
        }
    }
    fn process_response_eof(
        self,
        cx: &mut JSContext,
        _: RequestId,
        response: std::result::Result<(), NetworkError>,
        _: ResourceFetchTiming,
    ) {
        self.complete(cx, response);
    }
    fn process_csp_violations(
        &mut self,
        cx: &mut JSContext,
        _: RequestId,
        violations: Vec<Violation>,
    ) {
        if let Some(state) = SESSIONS.with(|s| s.borrow().get(&self.pipeline).cloned()) {
            if let Ok(doc) = state.registry.borrow().document() {
                doc.global()
                    .report_csp_violations(cx, violations, None, None);
            }
        }
    }
}
pub(super) fn abort_state(state: &Session) {
    let requests = std::mem::take(&mut state.module_load.borrow_mut().pending);
    if let Ok(doc) = state.registry.borrow().document() {
        let global = doc.global();
        for lease in requests.values() {
            if let Some(controller) = global.fetch_group_mut().fetch_controller(&lease.id) {
                controller.abort();
            }
            global.fetch_group_mut().fetch_records.remove(&lease.id);
        }
    }
    drop(requests);
}
pub(super) fn fail(cx: &mut JSContext, doc: &Document, state: &Session, error: String) {
    if state.module_load.borrow().failed.is_some() {
        return;
    }
    state.module_load.borrow_mut().failed = Some(error);
    abort_state(state);
    finish_blocker(cx, doc, state);
    queue_observation(doc);
}
fn queue_start(doc: &Document, state: &Session) {
    {
        let mut load = state.module_load.borrow_mut();
        if load.started || load.waiting || load.failed.is_some() || !load.pending.is_empty() {
            return;
        }
        load.waiting = true;
    }
    let document = Trusted::new(doc);
    doc.global().task_manager().dom_manipulation_task_source().queue(task!(MwrModuleStart: move |cx|{
        let doc=document.root();let Some(state)=session(doc.window())else{return;};state.module_load.borrow_mut().waiting=false;
        if doc.ReadyState()==DocumentReadyState::Loading{queue_start(&doc,&state);return;}
        let global=doc.global();
        run_a_script::<DomTypeHolder,_,_>(cx,&global,|cx|{
            let mut realm=crate::realms::enter_auto_realm(cx,doc.window());let mut cx=realm.current_realm();
            let(entry,sources)={let mut load=state.module_load.borrow_mut();load.started=true;(load.entry.clone(),load.sources.values().cloned().collect::<Vec<_>>())};
            let Some(dormant)=state.engine.borrow_mut().take()else{fail(&mut cx,&doc,&state,"module VM reentry".into());return;};
            let mut engine=dormant.resume(Box::new(NativeAdapter::new(&mut cx,state.registry.clone())));
            let result=engine.execute_modules(&entry,&sources);*state.engine.borrow_mut()=Some(engine.suspend());
            if let Err(error)=result{fail(&mut cx,&doc,&state,format!("module link/evaluation: {error:?}"));}
            finish_blocker(&mut cx,&doc,&state);
            queue_observation(&doc);
        });
    }));
}
pub(super) fn queue_observation(doc: &Document) {
    if session(doc.window()).is_none_or(|s| s.module_load.borrow().entry.is_empty()) {
        return;
    }
    let document = Trusted::new(doc);
    doc.global().task_manager().dom_manipulation_task_source().queue(task!(MwrModuleObserve: move |cx|{
        let doc=document.root();let Some(state)=session(doc.window())else{return;};
        let(entry,error,reported,started,count)={let l=state.module_load.borrow();(l.entry.clone(),l.failed.clone(),l.reported,l.started,l.sources.len())};
        if reported{return;}
        let status=if let Some(error)=error{ModuleStatus::Errored(error)}else if started{
            match state.engine.borrow_mut().as_mut().map(|e|e.module_status(&entry)){Some(Ok(status))=>status,_=>return}
        }else{return;};
        if status==ModuleStatus::Evaluating{return;}
        state.module_load.borrow_mut().reported=true;
        let target=doc.QuerySelector(cx,DOMString::from("#result")).ok().flatten();
        let text=target.and_then(|e|e.upcast::<Node>().GetTextContent()).map(|s|s.to_string()).unwrap_or_default();
        eprintln!("MWR_NATIVE_MODULE_PROOF {}",serde_json::json!({"status":format!("{status:?}"),"modules":count,"text":text}));
        queue_collection(&doc);
    }));
}

fn finish_blocker(cx: &mut JSContext, doc: &Document, state: &Session) {
    let blocker = state.module_load.borrow_mut().blocker.take();
    if let Some(url) = blocker {
        doc.finish_load(LoadType::Script(url), cx);
    }
}

impl SourceListener {
    fn complete(mut self, cx: &mut JSContext, response: std::result::Result<(), NetworkError>) {
        let Some(state) = SESSIONS.with(|s| s.borrow().get(&self.pipeline).cloned()) else {
            eprintln!("MWR_NATIVE_STALE_MODULE_SOURCE");
            return;
        };
        let lease = state.module_load.borrow_mut().pending.remove(&self.url);
        if let Some(lease) = lease {
            lease.finish();
        }
        let Ok(doc) = state.registry.borrow().document() else {
            return;
        };
        if let Err(error) = response {
            self.error = Some(format!("module body error: {error:?}"));
        }
        let result = (|| {
            if let Some(error) = self.error {
                return Err(error);
            }
            let metadata = self.metadata.take().ok_or("missing module metadata")?;
            let policy: net_traits::ReferrerPolicy = metadata
                .headers
                .and_then(|h| h.typed_get::<ReferrerPolicyHeader>())
                .into();
            if policy != net_traits::ReferrerPolicy::EmptyString {
                self.options.referrer_policy = policy;
            }
            let mime: Option<mime::Mime> = metadata.content_type.map(|s| s.into_inner().into());
            if !mime.as_ref().is_some_and(MimeClassifier::is_javascript) {
                return Err("module requires a JavaScript MIME type".into());
            }
            let (source, _) = UTF_8.decode_with_bom_removal(&self.data);
            add_source(
                cx,
                &doc,
                &state,
                self.url,
                metadata.final_url,
                source.into_owned(),
                self.options,
            )
        })();
        if let Err(error) = result {
            fail(cx, &doc, &state, error);
        }
    }
}

pub(super) fn dispatch_retired_source(cx: &mut JSContext, pipeline: PipelineId) {
    SourceListener {
        pipeline,
        url: "retired".into(),
        options: ScriptFetchOptions::default_classic_script(),
        data: vec![],
        metadata: None,
        error: None,
    }
    .complete(cx, Ok(()));
}
