//! test262 agents (INTERPRETING.md `$262.agent`): every started agent is a thread with its own
//! `Engine` (own heap and GC), sharing only SharedArrayBuffer data blocks with the others
//! (ECMA-262 §9.7-§9.8). Harness only: `$262` exists only with `enable_test_harness`.
//! Design: AGENTS.md.
use super::{
    shared_block::{Broadcast, Event, Mailbox},
    vm::{Engine, ScriptError, Value},
};
use crate::{dual::ExternalRoot, Model};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, ScriptError>;

/// State every agent of one cluster shares.
pub(super) struct Cluster {
    reports: Mutex<VecDeque<String>>,
    /// Uncaught errors that ended an agent, surfaced by the main agent's `getReport`.
    errors: Mutex<Vec<String>>,
    epoch: Instant,
}

pub(super) struct ChildHandle {
    mailbox: Arc<Mailbox>,
    alive: Arc<AtomicBool>,
}

pub(super) enum AgentLink {
    /// No agent was started (and this engine is not one).
    None,
    /// The main agent, after its first `$262.agent.start`.
    Parent {
        cluster: Arc<Cluster>,
        children: Vec<ChildHandle>,
    },
    /// A started agent.
    Child {
        cluster: Arc<Cluster>,
        callback: Option<ExternalRoot>,
        leaving: bool,
    },
}

/// What a child engine inherits from the main agent's configuration.
struct ChildConfig {
    step_limit: usize,
    mode: super::EngineMode,
    stress: bool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn describe(err: &ScriptError) -> String {
    match err {
        ScriptError::Thrown(ex) => match &ex.name {
            Some(name) => format!("{name}: {}", ex.message),
            None => ex.message.clone(),
        },
        other => format!("{other:?}"),
    }
}

impl Engine<'_> {
    pub(super) fn is_child_agent(&self) -> bool {
        matches!(self.atomics.agent, AgentLink::Child { .. })
    }
    /// Whether another agent could ever notify this one.
    pub(super) fn has_peer_agents(&self) -> bool {
        match &self.atomics.agent {
            AgentLink::None => false,
            AgentLink::Parent { children, .. } => {
                children.iter().any(|c| c.alive.load(Ordering::SeqCst))
            }
            AgentLink::Child { .. } => true,
        }
    }
    pub(super) fn release_agent_roots(&mut self) {
        if let AgentLink::Child { callback, .. } = &mut self.atomics.agent {
            if let Some(root) = callback.take() {
                let _ = self.runtime.unroot(root);
            }
        }
    }

    pub(super) fn agent_builtin(&mut self, name: &str, a: Value, b: Value) -> Result<Value> {
        match name {
            "$262.agent.start" => {
                let source = self.to_string(a)?;
                self.start_agent(source)
            }
            "$262.agent.broadcast" => self.broadcast(a, b),
            "$262.agent.getReport" => {
                let cluster = match &self.atomics.agent {
                    AgentLink::Parent { cluster, .. } => cluster.clone(),
                    _ => return Ok(Value::Null),
                };
                if let Some(report) = lock(&cluster.reports).pop_front() {
                    return Ok(Value::String(report.as_str().into()));
                }
                let errors = lock(&cluster.errors).clone();
                if let Some(error) = errors.first() {
                    // The harness polls getReport until it is non-null; an agent that died
                    // will never report, so fail now instead of at the runner timeout.
                    return self.throw_error("Error", &format!("agent failed: {error}"));
                }
                Ok(Value::Null)
            }
            "$262.agent.receiveBroadcast" => {
                if !self.callable(&a) {
                    return self.throw_error("TypeError", "receiveBroadcast needs a function");
                }
                let root = self.runtime.root(a.id().unwrap())?;
                let old = match &mut self.atomics.agent {
                    AgentLink::Child { callback, .. } => callback.replace(root),
                    _ => Some(root),
                };
                if let Some(old) = old {
                    self.runtime.unroot(old)?;
                }
                Ok(Value::Undefined)
            }
            "$262.agent.report" => {
                // INTERPRETING.md: "converted to a string" (explicitly, ToString).
                let message = self.to_string(a)?;
                if let AgentLink::Child { cluster, .. } = &self.atomics.agent {
                    lock(&cluster.reports).push_back(message);
                }
                Ok(Value::Undefined)
            }
            "$262.agent.leaving" => {
                if let AgentLink::Child { leaving, .. } = &mut self.atomics.agent {
                    *leaving = true;
                }
                Ok(Value::Undefined)
            }
            _ => Err(ScriptError::Unsupported("test262 agent operation")),
        }
    }

    /// `$262.agent.start(source)`: run `source` in a new agent; block until it is running
    /// (its top-level script has been evaluated).
    fn start_agent(&mut self, source: String) -> Result<Value> {
        if matches!(self.atomics.agent, AgentLink::None) {
            self.atomics.agent = AgentLink::Parent {
                cluster: Arc::new(Cluster {
                    reports: Mutex::default(),
                    errors: Mutex::default(),
                    epoch: self.atomics.epoch,
                }),
                children: vec![],
            };
        }
        let AgentLink::Parent { cluster, .. } = &self.atomics.agent else {
            return Err(ScriptError::Unsupported("nested test262 agents"));
        };
        let cluster = cluster.clone();
        let mailbox = Arc::new(Mailbox::default());
        let alive = Arc::new(AtomicBool::new(true));
        let config = ChildConfig {
            step_limit: self.step_limit(),
            mode: self.engine_mode(),
            stress: self.gc_policy() == super::GcPolicy::Stress,
        };
        let (ready, running) = mpsc::channel();
        let spawned = {
            let (mailbox, alive) = (mailbox.clone(), alive.clone());
            std::thread::Builder::new()
                .name("test262-agent".into())
                .stack_size(16 << 20)
                .spawn(move || run_child(source, config, cluster, mailbox, alive, ready))
        };
        if spawned.is_err() {
            return self.throw_error("Error", "cannot start an agent thread");
        }
        let _ = running.recv();
        if let AgentLink::Parent { children, .. } = &mut self.atomics.agent {
            children.push(ChildHandle { mailbox, alive });
        }
        Ok(Value::Undefined)
    }

    /// `$262.agent.broadcast(sab[, number])`: post to every running agent and block until
    /// each has taken the message (or has ended).
    fn broadcast(&mut self, sab: Value, number: Value) -> Result<Value> {
        let store = self.buffer_slot(sab)?;
        let (block, maximum) = {
            let b = store.borrow();
            match b.bytes.block() {
                Some(block) => (block.clone(), b.maximum),
                None => {
                    drop(b);
                    return self.throw_error("TypeError", "broadcast needs a SharedArrayBuffer");
                }
            }
        };
        let number = match number {
            Value::Undefined => f64::NAN,
            Value::Number(_) => self.to_number(number)?,
            _ => {
                return Err(ScriptError::Unsupported(
                    "test262 agent broadcast of a BigInt",
                ))
            }
        };
        let AgentLink::Parent { children, .. } = &self.atomics.agent else {
            return Ok(Value::Undefined);
        };
        let ack = Arc::new((Mutex::new(0usize), Condvar::new()));
        for child in children.iter().filter(|c| c.alive.load(Ordering::SeqCst)) {
            child.mailbox.post(Event::Broadcast(Broadcast {
                block: block.clone(),
                maximum,
                number,
                ack: ack.clone(),
            }));
        }
        let mut taken = lock(&ack.0);
        loop {
            let live = children
                .iter()
                .filter(|c| c.alive.load(Ordering::SeqCst))
                .count();
            if *taken >= live {
                break;
            }
            taken = ack
                .1
                .wait_timeout(taken, Duration::from_millis(10))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        Ok(Value::Undefined)
    }

    /// A broadcast taken from the mailbox: acknowledge it, then call the agent's
    /// `receiveBroadcast` callback with a SharedArrayBuffer over the same block.
    pub(super) fn receive_broadcast(&mut self, message: Broadcast) -> Result<()> {
        {
            let mut taken = lock(&message.ack.0);
            *taken += 1;
            message.ack.1.notify_all();
        }
        let callback = match &self.atomics.agent {
            AgentLink::Child {
                callback: Some(root),
                ..
            } => *root,
            _ => return Ok(()),
        };
        let f = self.runtime.resolve(callback)?;
        self.frame(|e| {
            e.protect(&Value::Object(f))?;
            let sab = e.shared_buffer_from_block(message.block, message.maximum)?;
            e.protect(&sab)?;
            let mut args = vec![sab];
            if !message.number.is_nan() {
                args.push(Value::number(message.number));
            }
            e.invoke_this(Value::Object(f), Value::Undefined, args)
                .map(|_| ())
        })
    }
}

/// The body of an agent thread: evaluate the script, then run the agent's event loop until
/// it has left (or has nothing left to wait for).
fn run_child(
    source: String,
    config: ChildConfig,
    cluster: Arc<Cluster>,
    mailbox: Arc<Mailbox>,
    alive: Arc<AtomicBool>,
    ready: mpsc::Sender<()>,
) {
    let fail = |message: String| lock(&cluster.errors).push(message);
    let mut e = match Engine::new(Model::Indirect, Model::Indirect) {
        Ok(e) => e,
        Err(err) => {
            fail(describe(&err));
            alive.store(false, Ordering::SeqCst);
            let _ = ready.send(());
            return;
        }
    };
    e.enable_test_harness();
    e.set_can_block(true);
    e.set_step_limit(config.step_limit);
    e.set_engine_mode(config.mode);
    e.set_stress(config.stress);
    e.atomics.epoch = cluster.epoch;
    e.atomics.mailbox = mailbox.clone();
    e.atomics.agent = AgentLink::Child {
        cluster: cluster.clone(),
        callback: None,
        leaving: false,
    };
    let mut result = e
        .execute(&source)
        .and_then(|_| e.microtask_checkpoint(10_000).map(|_| ()));
    let _ = ready.send(());
    while result.is_ok() {
        if let Some(deadline) = e.next_host_job() {
            e.wait_for_host_event(deadline);
            result = e
                .run_host_jobs()
                .and_then(|_| e.microtask_checkpoint(10_000).map(|_| ()));
            continue;
        }
        let (has_callback, leaving) = match &e.atomics.agent {
            AgentLink::Child {
                callback, leaving, ..
            } => (callback.is_some(), *leaving),
            _ => (false, true),
        };
        if (leaving || !has_callback) && !e.has_pending_waiters() {
            break;
        }
        // Wait for a broadcast or a notification.
        e.wait_for_host_event(Instant::now() + Duration::from_millis(100));
    }
    // Purge this agent's waiters from every block first, so that once the main agent sees
    // the failure (or `alive == false`) no notify can count a waiter that will never run.
    e.release_globals();
    if let Err(err) = &result {
        fail(describe(err));
    }
    alive.store(false, Ordering::SeqCst);
}
