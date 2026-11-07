//! Native GC stress harness; separate from the embedding's tracing protocol.
use super::*;
use crate::event_loop::document_loader::LoadType;

pub(super) struct Progress {
    completed: usize,
    limit: usize,
    errors: usize,
    baseline: (usize, usize, usize),
    pending: Option<(WeakNode, WeakNode, u64)>,
    drain_attempts: usize,
}

fn with_engine<T>(
    cx: &mut JSContext,
    state: &Session,
    f: impl FnOnce(&mut Engine<'_>) -> Result<T>,
) -> Result<T> {
    let dormant = state.engine.borrow_mut().take().ok_or("test VM busy")?;
    let mut engine = dormant.resume(Box::new(NativeAdapter::new(cx, state.registry.clone())));
    let result = f(&mut engine);
    *state.engine.borrow_mut() = Some(engine.suspend());
    result
}
fn counts(cx: &mut JSContext, state: &Session) -> Result<(usize, usize, usize)> {
    with_engine(cx, state, |e| {
        if e.live_roots() != 1 {
            return Err("execution roots escaped a call".into());
        }
        if e.metadata_objects() != e.live_objects() {
            return Err("metadata drift".into());
        }
        if e.text_content("detached").map_err(|e| format!("{e:?}"))? != "Rooted" {
            return Err("JS-rooted detached native node was lost".into());
        }
        Ok((
            e.live_objects(),
            e.remote_node_count(),
            e.metadata_objects(),
        ))
    })
}
pub(super) fn start(doc: &Document) {
    doc.loader_mut().add_blocking_load(LoadType::Script(doc.url()));
    let Some(state) = session(doc.window()) else {
        return;
    };
    let limit = std::env::var("MWR_NATIVE_GC_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1500);
    *state.stress.borrow_mut() = Some(Progress {
        completed: 0,
        limit,
        errors: 0,
        baseline: (0, 0, 0),
        pending: None,
        drain_attempts: 0,
    });
    queue_step(doc);
}
fn queue_step(doc: &Document) {
    let document = Trusted::new(doc);
    doc.global()
        .task_manager()
        .dom_manipulation_task_source()
        .queue(task!(MwrCycleStress: move |cx| {
            let document = document.root();
            let mut realm = crate::realms::enter_auto_realm(cx, document.window());
            if let Err(error) = step(&mut realm.current_realm(), &document) {
                eprintln!("MWR_NATIVE_ERROR joint GC: {error}");
                document.finish_load(LoadType::Script(document.url()), &mut realm.current_realm());
            }
        }));
}
// Every native root allocated in this helper is gone BEFORE collect_joint.
fn install(
    cx: &mut JSContext,
    doc: &Document,
    state: &Session,
    fail: bool,
) -> Result<(WeakNode, WeakNode, u64)> {
    let button = doc
        .CreateElement(
            cx,
            DOMString::from("button"),
            StringOrElementCreationOptions::String(DOMString::new()),
        )
        .map_err(|e| format!("{e:?}"))?;
    button.SetId(cx, DOMString::from("cycle"));
    button
        .upcast::<Node>()
        .SetTextContent(cx, Some(DOMString::from("Pulsar")))
        .map_err(|e| format!("{e:?}"))?;
    let body = doc.GetBody().ok_or("no body")?;
    body.upcast::<Node>()
        .AppendChild(cx, button.upcast::<Node>())
        .map_err(|e| format!("{e:?}"))?;
    let call = with_engine(cx, state, |e| {
        e.call_global(if fail { "makeError" } else { "make" })
            .map_err(|e| format!("{e:?}"))
    });
    if call.is_err() != fail {
        return Err(format!("unexpected setup result: {call:?}"));
    }
    let registry = state.registry.borrow();
    let target = WeakNode::new(button.upcast::<Node>());
    let id = registry
        .nodes
        .iter()
        .find(|(_, weak)| {
            weak.root()
                .is_some_and(|node| &*node == button.upcast::<Node>())
        })
        .map(|(id, _)| *id)
        .ok_or("target wrapper missing")?;
    let companion = registry
        .nodes
        .values()
        .find(|weak| {
            weak.root()
                .is_some_and(|node| node.GetTextContent().is_some_and(|s| s.to_string() == "Captured"))
        })
        .cloned()
        .ok_or("captured native companion missing")?;
    Ok((target, companion, id))
}
fn click_and_remove(cx: &mut JSContext, target: &WeakNode, companion: &WeakNode) -> Result<()> {
    let target = target.root().ok_or("DOM-rooted target collected")?;
    let companion = companion
        .root()
        .ok_or("DOM -> VM -> native edge was lost")?;
    target
        .downcast::<HTMLElement>()
        .ok_or("not HTML")?
        .Click(cx);
    if target.GetTextContent().is_none_or(|s| s.to_string() != "Funciona")
        || companion.GetTextContent().is_none_or(|s| s.to_string() != "Callback")
    {
        return Err("closure did not mutate both real native nodes".into());
    }
    let parent = target.GetParentNode().ok_or("target detached early")?;
    parent
        .RemoveChild(cx, &target)
        .map_err(|e| format!("{e:?}"))?;
    Ok(())
}
fn step(cx: &mut JSContext, doc: &Document) -> Result<()> {
    let state = session(doc.window()).ok_or("stress session missing")?;
    let mut progress = state
        .stress
        .borrow_mut()
        .take()
        .ok_or("stress state missing")?;
    if let Some((target, companion, stale)) = progress.pending.take() {
        // Attribute/restyle queues are legitimate native roots until layout
        // consumes them. Flush normal layout work before asking about garbage.
        doc.window().reflow(cx, layout_api::ReflowGoal::LayoutQuery(
            layout_api::QueryMsg::FlushForUpdateTheRenderingQuery));
        collect_joint(cx, doc.window())?;
        // Weak token expiration is caused by Servo's native finalizer, not by
        // removing our registry entry, checking is_connected or dropping the VM.
        if target.alive.upgrade().is_some() || companion.alive.upgrade().is_some() {
            progress.drain_attempts += 1;
            if progress.drain_attempts >= 5 {
                return Err(format!("native cycle survived {} collections at iteration {}; roots={:?}, native_marks={:?}",
                    progress.drain_attempts, progress.completed,
                    state.engine.borrow().as_ref().unwrap().trace_native_edges(None),
                    *state.marked.borrow()));
            }
            progress.pending = Some((target, companion, stale));
            *state.stress.borrow_mut() = Some(progress);
            queue_step(doc);
            return Ok(());
        }
        progress.drain_attempts = 0;
        if counts(cx, &state)? != progress.baseline {
            return Err("VM did not return to live realm baseline".into());
        }
        if state.registry.borrow().nodes.len() != 1 || !state.registry.borrow().pins.is_empty() {
            return Err("registry or invocation pins retained a cycle".into());
        }
        with_engine(cx, &state, |e| {
            if e.dispatch_remote_click(stale).is_ok() {
                return Err("stale native ID resurrected".into());
            }
            Ok(())
        })?;
        progress.completed += 1;
        if progress.completed % 100 == 0 {
            eprintln!("MWR_GC_PROGRESS {}", progress.completed);
        }
        if progress.completed == progress.limit {
            eprintln!(
                "MWR_GC_PROOF {}",
                serde_json::json!({
                    "iterations": progress.completed,
                    "native_buttons_finalized": progress.completed * 2,
                    "callbacks": state.callbacks.get(),
                    "recovered_errors": progress.errors,
                    "baseline_vm_objects": progress.baseline.0,
                    "final_vm_objects": progress.baseline.0,
                    "persistent_native_handles": 1,
                    "escaped_invocation_pins": 0,
                    "manual_global_release": false,
                    "native_to_vm_to_native_survival": true,
                    "disconnected_js_root_survival": true,
                    "stale_ids_rejected": true,
                })
            );
            doc.QuerySelector(cx, DOMString::from("#result"))
                .map_err(|e| format!("{e:?}"))?
                .ok_or("result element missing")?
                .upcast::<Node>()
                .SetTextContent(cx, Some(DOMString::from("Ciclos recogidos")))
                .map_err(|e| format!("{e:?}"))?;
            doc.finish_load(LoadType::Script(doc.url()), cx);
            return Ok(());
        }
    }
    if progress.completed == 0 && progress.baseline == (0, 0, 0) {
        collect_joint(cx, doc.window())?;
        progress.baseline = counts(cx, &state)?;
        if progress.baseline.1 != 1 {
            return Err("unexpected detached root baseline".into());
        }
    }
    let fail = progress.completed % 17 == 0;
    let probes = install(cx, doc, &state, fail)?;
    if fail {
        progress.errors += 1;
    }
    // No stack root from install survives here. The target must live through the
    // DOM; its companion must live through the foreign callback's VM environment.
    collect_joint(cx, doc.window())?;
    click_and_remove(cx, &probes.0, &probes.1)?;
    progress.pending = Some(probes);
    *state.stress.borrow_mut() = Some(progress);
    queue_step(doc);
    Ok(())
}
