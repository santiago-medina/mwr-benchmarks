//! Filesystem test host. Browser module URL/security policy belongs to Servo.
use gc_lab::{
    script::{parser::parse_module, wire, Engine, GcPolicy, ModuleSource, ModuleStatus},
    Model,
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
fn load(
    path: PathBuf,
    root: &Path,
    seen: &mut HashSet<PathBuf>,
    sources: &mut Vec<ModuleSource>,
) -> Result<(), String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err("module escapes fixture root".into());
    }
    if !seen.insert(path.clone()) {
        return Ok(());
    }
    if seen.len() > 128 {
        return Err("module graph limit".into());
    }
    let source = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let syntax = parse_module(&source).map_err(|e| format!("{e:?}"))?;
    let mut resolutions = vec![];
    for specifier in syntax.dependencies() {
        if !specifier.starts_with('.') {
            return Err("fixture host supports relative paths only".into());
        }
        let target = path
            .parent()
            .unwrap()
            .join(specifier)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        resolutions.push((specifier.to_string(), target.to_string_lossy().into_owned()));
        load(target, root, seen, sources)?;
    }
    sources.push(ModuleSource {
        url: path.to_string_lossy().into_owned(),
        source,
        resolutions,
    });
    Ok(())
}
fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("module_check ROOT ENTRY"))
        .canonicalize()
        .unwrap();
    let entry = root
        .join(args.next().expect("entry"))
        .canonicalize()
        .unwrap();
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.set_gc_policy(GcPolicy::Threshold);
    e.enable_test_harness();
    let result = (|| -> Result<(String, String), String> {
        let mut sources = vec![];
        load(entry.clone(), &root, &mut HashSet::new(), &mut sources)?;
        let entry = entry.to_string_lossy();
        e.execute_modules(&entry, &sources)
            .map_err(|e| format!("{e:?}"))?;
        e.microtask_checkpoint(10000)
            .map_err(|e| format!("{e:?}"))?;
        if let Some(error) = e.unhandled_test_failure() {
            return Err(error);
        }
        let errors = e.take_microtask_errors();
        if !errors.is_empty() {
            return Err(format!("{errors:?}"));
        }
        Ok(
            match e.module_status(&entry).map_err(|e| format!("{e:?}"))? {
                ModuleStatus::Evaluated => (
                    "evaluated".into(),
                    e.module_binding_string(&entry, "result")
                        .unwrap_or_default(),
                ),
                ModuleStatus::Evaluating => ("pending".into(), String::new()),
                ModuleStatus::Errored(error) => ("errored".into(), error),
            },
        )
    })();
    e.release_globals();
    let clean = (
        e.live_objects(),
        e.metadata_objects(),
        e.live_roots(),
        e.charged_bytes(),
    ) == (0, 0, 0, 0);
    let (status, text) = if !clean {
        ("failed".into(), "GC cleanup failed".into())
    } else {
        result.unwrap_or_else(|e| ("failed".into(), e))
    };
    println!("{status}\t{}", wire::encode(&text));
    std::process::exit(if status == "evaluated" { 0 } else { 1 });
}
