//! Module linking keeps aliases in ordinary traced lexical environments.
use super::{
    parser::{self, Module},
    promises::{Capability, Internal, State},
    vm::{Engine, Object, ScriptError, Value},
};
use crate::dual::Id;
use std::collections::{HashMap, HashSet};
type Result<T> = std::result::Result<T, ScriptError>;
#[derive(Clone, Debug)]
pub struct ModuleSource {
    pub url: String,
    pub source: String,
    pub resolutions: Vec<(String, String)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleStatus {
    Evaluating,
    Evaluated,
    Errored(String),
}
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Waiting,
    Running,
    Done,
}
struct Record {
    url: String,
    syntax: Module,
    deps: Vec<usize>,
    env: Option<Id>,
    cap: Option<Capability>,
    phase: Phase,
    group: usize,
    rank: usize,
}
#[derive(Default)]
pub(super) struct Registry {
    records: Vec<Record>,
    names: HashMap<String, usize>,
    order: Vec<usize>,
    groups: Vec<Vec<usize>>,
    errors: HashMap<usize, Value>,
    limit: usize,
    entry: usize,
    pumping: bool,
}
impl Registry {
    fn visit(
        &mut self,
        url: &str,
        sources: &HashMap<&str, &ModuleSource>,
        depth: usize,
    ) -> Result<usize> {
        if let Some(i) = self.names.get(url) {
            return Ok(*i);
        }
        if depth >= 128 || self.records.len() >= 128 {
            return Err(ScriptError::Runtime("module graph limit"));
        }
        let source = sources
            .get(url)
            .ok_or_else(|| ScriptError::Host(format!("missing module source: {url}")))?;
        let syntax = parser::parse_module(&source.source).map_err(ScriptError::Syntax)?;
        let requests = syntax
            .dependencies()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let i = self.records.len();
        self.names.insert(url.into(), i);
        // The placeholder is replaced before instantiation; it never enters the VM graph.
        self.records.push(Record {
            url: url.into(),
            syntax,
            deps: vec![],
            env: None,
            cap: None,
            phase: Phase::Waiting,
            group: 0,
            rank: 0,
        });
        for request in requests {
            let target = source
                .resolutions
                .iter()
                .find(|(s, _)| s == &request)
                .map(|(_, u)| u.as_str())
                .ok_or_else(|| {
                    ScriptError::Host(format!("unresolved module specifier: {request}"))
                })?;
            let dep = self.visit(target, sources, depth + 1)?;
            self.records[i].deps.push(dep);
        }
        self.records[i].rank = self.order.len();
        self.order.push(i);
        Ok(i)
    }
    fn components(&mut self) {
        fn assign(
            i: usize,
            g: usize,
            reverse: &[Vec<usize>],
            seen: &mut HashSet<usize>,
            groups: &mut Vec<Vec<usize>>,
        ) {
            if !seen.insert(i) {
                return;
            }
            groups[g].push(i);
            for &j in &reverse[i] {
                assign(j, g, reverse, seen, groups);
            }
        }
        let mut reverse = vec![vec![]; self.records.len()];
        for (i, r) in self.records.iter().enumerate() {
            for &j in &r.deps {
                reverse[j].push(i);
            }
        }
        let mut seen = HashSet::new();
        for &i in self.order.iter().rev() {
            if !seen.contains(&i) {
                let g = self.groups.len();
                self.groups.push(vec![]);
                assign(i, g, &reverse, &mut seen, &mut self.groups);
            }
        }
        for (g, members) in self.groups.iter().enumerate() {
            for &i in members {
                self.records[i].group = g;
            }
        }
        self.limit = self.order.len();
    }
    fn resolve(
        &self,
        i: usize,
        name: &str,
        seen: &mut HashSet<(usize, String)>,
    ) -> Option<(usize, String)> {
        if !seen.insert((i, name.into())) {
            return None;
        }
        let r = &self.records[i];
        let export = r.syntax.exports.iter().find(|e| e.exported == name)?;
        if let Some(from) = &export.from {
            let pos = r.syntax.dependencies().iter().position(|s| *s == from)?;
            return self.resolve(r.deps[pos], &export.local, seen);
        }
        for import in &r.syntax.imports {
            if let Some((imported, _)) = import
                .bindings
                .iter()
                .find(|(_, local)| local == &export.local)
            {
                let pos = r
                    .syntax
                    .dependencies()
                    .iter()
                    .position(|s| *s == import.specifier)?;
                return self.resolve(r.deps[pos], imported, seen);
            }
        }
        Some((i, export.local.clone()))
    }
}
impl Engine<'_> {
    pub fn execute_modules(&mut self, entry: &str, sources: &[ModuleSource]) -> Result<()> {
        if sources.len() > 128 || sources.iter().map(|s| s.source.len()).sum::<usize>() > 1048576 {
            return Err(ScriptError::Runtime("module source limit"));
        }
        let mut input = HashMap::new();
        for s in sources {
            if input.insert(s.url.as_str(), s).is_some() {
                return Err(ScriptError::Runtime("duplicate module URL"));
            }
        }
        let mut graph = Registry::default();
        graph.entry = graph.visit(entry, &input, 0)?;
        graph.components();
        self.execute("")?;
        self.modules = graph;
        let result = self.frame(|e| {
            e.link_modules()?;
            e.pump_modules()
        });
        if result.is_err() {
            self.release_globals();
        } else {
            self.collect();
        }
        result
    }
    fn link_modules(&mut self) -> Result<()> {
        let global = self.runtime.resolve(self.global.unwrap())?;
        for i in 0..self.modules.records.len() {
            let body = self.modules.records[i].syntax.body.clone();
            let env = self.new_environment(Some(global), &body, true)?;
            self.runtime.link(global, env)?;
            self.modules.records[i].env = Some(env);
            self.initialize(env, "this", Value::Undefined)?;
            let cap = self.capability(Value::Object(
                self.intrinsics.as_ref().unwrap().functions["Promise"],
            ))?;
            for (n, v) in cap.values().into_iter().enumerate() {
                self.initialize(env, &format!("\0moduleCap{n}"), v)?;
            }
            self.modules.records[i].cap = Some(cap);
            self.hoist_functions(&body, env)?;
        }
        for i in 0..self.modules.records.len() {
            let imports = self.modules.records[i].syntax.imports.clone();
            let exports = self.modules.records[i].syntax.exports.clone();
            for export in exports {
                let Some((target, name)) =
                    self.modules
                        .resolve(i, &export.exported, &mut HashSet::new())
                else {
                    return self.throw_error("SyntaxError", "unresolved cyclic export");
                };
                if !matches!(self.objects.get(&self.modules.records[target].env.unwrap()),Some(Object::Environment{bindings,..})if bindings.contains_key(&name))
                {
                    return self.throw_error("SyntaxError", "export has no local binding");
                }
            }
            for import in imports {
                let pos = self.modules.records[i]
                    .syntax
                    .dependencies()
                    .iter()
                    .position(|s| *s == import.specifier)
                    .unwrap();
                let dep = self.modules.records[i].deps[pos];
                for (imported, local) in import.bindings {
                    let Some((target, name)) =
                        self.modules.resolve(dep, &imported, &mut HashSet::new())
                    else {
                        return self.throw_error("SyntaxError", "imported export does not exist");
                    };
                    let env = self.modules.records[i].env.unwrap();
                    let target = self.modules.records[target].env.unwrap();
                    self.runtime.link(env, target)?;
                    if let Some(Object::Environment { aliases, .. }) = self.objects.get_mut(&env) {
                        aliases.insert(local, (target, name));
                    }
                }
            }
        }
        Ok(())
    }
    fn fail_module_group(&mut self, group: usize, error: Value) -> Result<()> {
        if self.modules.errors.contains_key(&group) {
            return Ok(());
        }
        let env = self.modules.records[self.modules.groups[group][0]]
            .env
            .unwrap();
        self.initialize(env, "\0moduleError", error.clone())?;
        self.modules.errors.insert(group, error);
        Ok(())
    }
    fn pump_modules(&mut self) -> Result<()> {
        if self.modules.pumping {
            return Ok(());
        }
        self.modules.pumping = true;
        let result = (|| {
            loop {
                let mut propagated = false;
                for i in 0..self.modules.records.len() {
                    let group = self.modules.records[i].group;
                    if self.modules.errors.contains_key(&group) {
                        continue;
                    }
                    for dep in self.modules.records[i].deps.clone() {
                        let dg = self.modules.records[dep].group;
                        if let Some(error) = self.modules.errors.get(&dg).cloned() {
                            self.fail_module_group(group, error)?;
                            propagated = true;
                            break;
                        }
                    }
                }
                if propagated {
                    continue;
                }
                let ready = self.modules.order.iter().copied().find(|&i| {
                    let r = &self.modules.records[i];
                    r.rank < self.modules.limit
                        && r.phase == Phase::Waiting
                        && !self.modules.errors.contains_key(&r.group)
                        && r.deps.iter().all(|&j| {
                            let d = &self.modules.records[j];
                            if d.group == r.group {
                                d.rank > r.rank || d.phase == Phase::Done
                            } else {
                                self.modules.groups[d.group]
                                    .iter()
                                    .all(|&m| self.modules.records[m].phase == Phase::Done)
                            }
                        })
                });
                let Some(i) = ready else {
                    break;
                };
                self.modules.records[i].phase = Phase::Running;
                let r = &self.modules.records[i];
                let env = r.env.unwrap();
                let cap = r.cap.clone().unwrap();
                let body = r.syntax.body.clone();
                let program = super::asynchronous::compile_module(&body)?;
                let promise = self.start_program(env, program, cap)?;
                let Value::Object(p) = promise else {
                    unreachable!()
                };
                match self.data(p)?.promise.as_ref().unwrap().state.clone() {
                    State::Fulfilled(_) => self.modules.records[i].phase = Phase::Done,
                    State::Rejected(error) => {
                        self.unhandled.remove(&p);
                        self.modules.limit =
                            self.modules.limit.min(self.modules.records[i].rank + 1);
                        self.fail_module_group(self.modules.records[i].group, error)?;
                    }
                    State::Pending => {
                        let yes = self.internal_function(
                            Internal::ModuleSettled {
                                index: i,
                                reject: false,
                            },
                            1,
                        )?;
                        let no = self.internal_function(
                            Internal::ModuleSettled {
                                index: i,
                                reject: true,
                            },
                            1,
                        )?;
                        self.attach_reaction(p, Some(yes), Some(no), None)?;
                    }
                }
            }
            Ok(())
        })();
        self.modules.pumping = false;
        result
    }
    pub(super) fn module_settled(
        &mut self,
        index: usize,
        value: Value,
        reject: bool,
    ) -> Result<Value> {
        if index >= self.modules.records.len() {
            return Err(ScriptError::Runtime("invalid module continuation"));
        }
        self.modules.records[index].phase = Phase::Done;
        if reject {
            self.fail_module_group(self.modules.records[index].group, value)?;
        }
        self.pump_modules()?;
        Ok(Value::Undefined)
    }
    pub fn loaded_modules(&self) -> Vec<String> {
        self.modules.records.iter().map(|r| r.url.clone()).collect()
    }
    pub fn module_status(&mut self, url: &str) -> Result<ModuleStatus> {
        let i = *self
            .modules
            .names
            .get(url)
            .ok_or(ScriptError::Runtime("unknown module"))?;
        let group = self.modules.records[i].group;
        if let Some(error) = self.modules.errors.get(&group).cloned() {
            return Ok(ModuleStatus::Errored(match self.thrown(error) {
                ScriptError::Thrown(ex) => format!(
                    "{}: {}",
                    ex.name.unwrap_or_else(|| "Error".into()),
                    ex.message
                ),
                other => format!("{other:?}"),
            }));
        }
        Ok(
            if self.modules.groups[group]
                .iter()
                .all(|&i| self.modules.records[i].phase == Phase::Done)
            {
                ModuleStatus::Evaluated
            } else {
                ModuleStatus::Evaluating
            },
        )
    }
    pub fn module_binding_string(&mut self, url: &str, name: &str) -> Result<String> {
        let i = *self
            .modules
            .names
            .get(url)
            .ok_or(ScriptError::Runtime("unknown module"))?;
        let env = self.modules.records[i].env.unwrap();
        self.frame(|e| {
            let value = e.lookup(env, name)?;
            e.to_string(value)
        })
    }
}
