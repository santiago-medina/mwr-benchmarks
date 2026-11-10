//! One Test262 script case per process (strict unless `--sloppy`); no source rewriting or helper stubs.
//! `--include=FILE` prepends an official harness include (as the reference runner does).
//! `--gc-stress` collects at every allocation (rooting debug); the default is the threshold policy.
//! `--module` runs the case as a module record (includes run first as a classic script) and
//! answers the engine's module requests from files resolved against the referrer's path.
//! `--negative-resolution` passes only when loading/linking throws the expected error type.
use gc_lab::{
    script::{
        parser, Engine, GcPolicy, ModuleLoad, ModuleRequest, ModuleStatus, Referrer, ScriptError,
    },
    Model,
};
use std::path::{Component, Path, PathBuf};

/// Lexical `dirname(referrer) + specifier`, normalised (test262 fixtures are relative paths).
fn resolve(base: &Path, specifier: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for c in base.parent().unwrap_or(base).join(specifier).components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// The test host's HostLoadImportedModule answer for one request.
fn load(case: &Path, request: &ModuleRequest) -> ModuleLoad {
    // test262 INTERPRETING.md: `<module source>` names a host module that has a
    // module source object; `<do not resolve>` must fail to load.
    if request.specifier == "<module source>" {
        return ModuleLoad::ModuleSource {
            url: "<module source>".into(),
            class_name: "ModuleSource".into(),
            host_defined: 0,
        };
    }
    let base = match &request.referrer {
        Referrer::Module { url, .. } => PathBuf::from(url),
        Referrer::Script { .. } | Referrer::Realm => case.to_path_buf(),
    };
    // The HTML host's module type: the "type" attribute, "javascript" without one.
    // Only JSON modules are implemented; `text` and `bytes` (import-text,
    // import-bytes proposals) and other types stay unsupported, never a pass.
    let module_type = match request.attributes.iter().find(|(k, _)| k == "type") {
        None => "javascript",
        Some((_, t)) if t == "json" => "json",
        Some((_, t)) => {
            return ModuleLoad::Error {
                kind: "SyntaxError".into(),
                message: format!("unsupported module type '{t}'"),
            }
        }
    };
    let path = resolve(&base, &request.specifier);
    match std::fs::read_to_string(&path) {
        Ok(source) => ModuleLoad::Source {
            url: path.to_string_lossy().into_owned(),
            module_type: module_type.into(),
            source,
            host_defined: 0,
        },
        Err(err) => ModuleLoad::Error {
            kind: "TypeError".into(),
            message: format!("Cannot load module '{}': {err}", request.specifier),
        },
    }
}
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("js_check CASE.js [expected runtime error]");
    let remaining: Vec<_> = args.collect();
    let asynchronous = remaining.iter().any(|s| s == "--async");
    let sloppy = remaining.iter().any(|s| s == "--sloppy");
    let gc_stress = remaining.iter().any(|s| s == "--gc-stress");
    let can_block = remaining.iter().any(|s| s == "--can-block");
    let mut includes = String::new();
    for file in remaining
        .iter()
        .filter_map(|s| s.strip_prefix("--include="))
    {
        includes.push_str(&std::fs::read_to_string(file).expect("include"));
        includes.push('\n');
    }
    let negative_parse = remaining.iter().any(|s| s == "--negative-parse");
    let strict_only = remaining.iter().any(|s| s == "--strict-only");
    let module = remaining.iter().any(|s| s == "--module");
    let negative_resolution = remaining.iter().any(|s| s == "--negative-resolution");
    // BYTECODE_VM.md stage 3: compilable function bodies run on the register VM (the
    // default here since checkpoint 32: full test262 run with 0 lost cases); `--engine=tree`
    // (or MWR_ENGINE=tree) keeps everything on the tree-walker, the embedding default.
    // `--vm-stats` prints the VM counters to stderr.
    let engine = remaining
        .iter()
        .find_map(|s| s.strip_prefix("--engine="))
        .map(str::to_string)
        .or_else(|| std::env::var("MWR_ENGINE").ok());
    let vm_engine = engine.as_deref() != Some("tree");
    let vm_stats = remaining.iter().any(|s| s == "--vm-stats");
    let negative = remaining.into_iter().find(|s| !s.starts_with("--"));
    let source = std::fs::read_to_string(&path).expect("source");
    if negative_parse {
        // Parse-phase negative case: never evaluated. Passes only when every applicable mode
        // (strict, sloppy, or both for unflagged cases) rejects it with a genuine early error;
        // rejections for unsupported syntax or implementation limits stay unsupported.
        let modes: &[bool] = if sloppy {
            &[false]
        } else if strict_only {
            &[true]
        } else {
            &[true, false]
        };
        let mut outcome = ("passed", String::new());
        if module {
            // Module code is always strict; the includes are separate scripts, not prepended.
            outcome = match parser::parse_module(&source) {
                Err(s) if s.message.contains("unsupported") || s.message.ends_with("limit") => {
                    ("unsupported", format!("syntax (module): {}", s.message))
                }
                Err(s) => ("passed", format!("early error: {}", s.message)),
                Ok(_) => (
                    "failed",
                    "parsed as a module; expected early SyntaxError".into(),
                ),
            };
        }
        for &strict in if module { &[][..] } else { modes } {
            let prefix = if strict { "\"use strict\";\n" } else { "" };
            let mode = if strict { "strict" } else { "sloppy" };
            match parser::parse(&format!("{prefix}{includes}{source}")) {
                Err(s) if s.message.contains("unsupported") || s.message.ends_with("limit") => {
                    outcome = ("unsupported", format!("syntax ({mode}): {}", s.message));
                    break;
                }
                Err(s) => outcome.1 = format!("early error: {}", s.message),
                Ok(_) => {
                    outcome = (
                        "failed",
                        format!("parsed in {mode} mode; expected early SyntaxError"),
                    );
                    break;
                }
            }
        }
        if outcome.0 == "passed" && negative.as_deref() != Some("SyntaxError") {
            outcome = ("unsupported", format!("negative parse type {negative:?}"));
        }
        println!(
            "{{\"status\":{},\"reason\":{},\"assertions\":0,\"cleanup\":true}}",
            quote(outcome.0),
            quote(&outcome.1)
        );
        std::process::exit(match outcome.0 {
            "passed" => 0,
            "unsupported" => 2,
            _ => 1,
        });
    }
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.enable_test_harness();
    // [[CanBlock]] of the main agent: test262's CanBlockIsTrue flag maps to --can-block.
    e.set_can_block(can_block);
    e.set_gc_policy(if gc_stress {
        GcPolicy::Stress
    } else {
        GcPolicy::Threshold
    });
    // The engine default (10k steps) is an embedding safety budget; conformance cases such as
    // testTypedArray.js loops need far more. The runner's per-case timeout bounds runaway cases.
    e.set_step_limit(50_000_000);
    if vm_engine {
        e.set_engine_mode(gc_lab::script::EngineMode::Vm);
    }
    let prefix = if sloppy { "" } else { "\"use strict\";\n" };
    let full = format!("{prefix}{ASSERT_GLOBALS}{includes}{source}");
    let case = std::fs::canonicalize(&path).expect("case path");
    let root = case.to_string_lossy().into_owned();
    let mut load_error = false;
    // A dependency's ParseModule error surfaces while loading (FinishLoadingImportedModule).
    let mut dependency_syntax = false;
    let mut result = if module {
        // INTERPRETING.md: includes are evaluated as global scripts before the module.
        e.execute(&format!("{ASSERT_GLOBALS}{includes}"))
            .and_then(|_| e.execute_module(&root, &source, 0))
    } else if source.starts_with("#!") {
        // §12.5: a HashbangComment is only valid at the start of the source text, so a case
        // that opens with `#!` (always `raw`) runs as its own Script after the helper globals.
        e.execute(&format!("{prefix}{ASSERT_GLOBALS}{includes}"))
            .and_then(|_| e.execute(&source))
    } else {
        e.execute(&full)
    };
    if module && result.is_err() {
        load_error = true;
    }
    if let (Err(ScriptError::Syntax(s)), Ok(_)) =
        (&result, std::env::var("JS_CHECK_SYNTAX_CONTEXT"))
    {
        // Debug aid for lexer/parser surveys: the offending character and its line.
        let at = s.offset.min(full.len());
        let start = full[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = full[at..].find('\n').map_or(full.len(), |i| at + i);
        let c = full[at..].chars().next().map_or(0, |c| c as u32);
        eprintln!("SYNTAX\t{}\tU+{c:04X}\t{}", s.message, &full[start..end]);
    }
    if result.is_ok() {
        result = e.microtask_checkpoint(10_000).map(|_| ());
    }
    // Host event loop: FinalizationRegistry cleanup tasks, then timers and Atomics waiter
    // jobs (setTimeout, waitAsync), each followed by a microtask checkpoint.
    while result.is_ok() {
        let requests = e.take_module_requests();
        if !requests.is_empty() {
            for request in requests {
                if let Err(err) = e.finish_module_request(request.id, load(&case, &request)) {
                    // A static graph's load/link failure surfaces here; import() rejects instead.
                    load_error = true;
                    dependency_syntax = matches!(err, ScriptError::Syntax(_));
                    result = Err(err);
                    break;
                }
            }
            if result.is_ok() {
                result = e.microtask_checkpoint(10_000).map(|_| ());
            }
        } else if e.pending_finalization_cleanups() != 0 {
            result = e
                .run_finalization_cleanup()
                .and_then(|_| e.microtask_checkpoint(10_000).map(|_| ()));
        } else if let Some(deadline) = e.next_host_job() {
            let now = std::time::Instant::now();
            if deadline > now {
                std::thread::sleep(deadline - now);
            }
            result = e
                .run_host_jobs()
                .and_then(|_| e.microtask_checkpoint(10_000).map(|_| ()));
        } else {
            break;
        }
    }
    // Evaluation errors of the root graph are recorded on the module record.
    let mut module_error: Option<(String, String)> = None;
    if module && result.is_ok() {
        match e.module_status(&root) {
            Ok(ModuleStatus::Errored(message)) => {
                let name = message.split(':').next().unwrap_or("Error").to_string();
                module_error = Some((name, message));
            }
            Ok(ModuleStatus::Evaluating) => {
                result = Err(ScriptError::Host("module evaluation did not settle".into()))
            }
            Ok(ModuleStatus::Evaluated) => {}
            Err(err) => result = Err(err),
        }
    }
    let (done, error) = e.async_test_state();
    let harness_error = if result.is_err() {
        None
    } else if let Some(error) = error {
        Some(format!("$DONE: {error}"))
    } else if asynchronous && done != 1 {
        Some(format!("expected one $DONE, got {done}"))
    } else if let Some(error) = e.unhandled_test_failure() {
        Some(error)
    } else {
        let errors = e.take_microtask_errors();
        if errors.is_empty() {
            None
        } else {
            Some(format!("microtask errors: {errors:?}"))
        }
    };

    let assertions = e.assertion_count();
    e.release_globals();
    let clean = e.live_objects() == 0
        && e.metadata_objects() == 0
        && e.live_roots() == 0
        && e.charged_bytes() == 0;
    let (status, reason) = if !clean {
        ("failed", "GC cleanup failed".into())
    } else if let Some(error) = harness_error {
        ("failed", error)
    } else {
        match (result, negative) {
            (Ok(()), expected) if module_error.is_some() => {
                let (name, message) = module_error.unwrap();
                if expected.as_deref() == Some(name.as_str()) && !negative_resolution {
                    ("passed", String::new())
                } else {
                    (
                        "failed",
                        format!("module evaluation: {message}; expected {expected:?}"),
                    )
                }
            }
            (Err(ScriptError::Syntax(s)), Some(expected))
                if negative_resolution
                    && dependency_syntax
                    && expected == "SyntaxError"
                    && !s.message.contains("unsupported")
                    && !s.message.ends_with("limit") =>
            {
                ("passed", String::new())
            }
            (Err(ScriptError::Syntax(s)), _) => ("unsupported", format!("syntax: {}", s.message)),
            (Err(ScriptError::Unsupported(s)), _) => ("unsupported", s.into()),
            (Err(ScriptError::Thrown(ex)), Some(expected))
                if ex.name.as_deref() == Some(expected.as_str())
                    && negative_resolution == load_error =>
            {
                ("passed", String::new())
            }
            (Ok(()), None) => ("passed", String::new()),
            (result, expected) => ("failed", format!("{result:?}; expected {expected:?}")),
        }
    };
    if vm_stats {
        let stats = e.vm_stats();
        let mut fallbacks: Vec<_> = stats.fallbacks.iter().collect();
        fallbacks.sort_by(|a, b| b.1.cmp(a.1));
        eprintln!(
            "vm: compiled={} calls={} elided={} max_registers={} fallbacks={fallbacks:?}",
            stats.compiled, stats.calls, stats.elided, stats.max_registers
        );
        let mut script_fallbacks: Vec<_> = stats.script_fallbacks.iter().collect();
        script_fallbacks.sort_by(|a, b| b.1.cmp(a.1));
        eprintln!(
            "vm-script: scripts={} fallbacks={script_fallbacks:?}",
            stats.scripts
        );
    }
    println!(
        "{{\"status\":{},\"reason\":{},\"assertions\":{assertions},\"cleanup\":{clean}}}",
        quote(status),
        quote(&reason)
    );
    std::process::exit(match status {
        "passed" => 0,
        "unsupported" => 2,
        _ => 1,
    });
}

/// The plain global helpers that test262's harness/assert.js defines next to `assert`
/// (`assert` itself is native via `enable_test_harness`). Upstream moved `compareArray`
/// and `isPrimitive` into assert.js, and harness files such as testTypedArray.js use them.
const ASSERT_GLOBALS: &str = r#"function isNegativeZero(value) { return value === 0 && 1 / value === -Infinity; }
function isPrimitive(value) { return !value || (typeof value !== 'object' && typeof value !== 'function'); }
function compareArray(a, b) {
  if (b.length !== a.length) return false;
  for (var i = 0; i < a.length; i++) {
    var x = b[i], y = a[i];
    if (x === y ? x === 0 && 1 / x !== 1 / y : x === x || y === y) return false;
  }
  return true;
}
compareArray.format = function (arrayLike) { return "[" + Array.prototype.map.call(arrayLike, String).join(", ") + "]"; };
function formatIdentityFreeValue(value) {
  switch (value === null ? 'null' : typeof value) {
    case 'string':
      return typeof JSON !== "undefined" ? JSON.stringify(value) : '"' + value + '"';
    case 'bigint':
      return String(value) + "n";
    case 'number':
      if (isNegativeZero(value)) return '-0';
    case 'boolean':
    case 'undefined':
    case 'null':
      return String(value);
  }
}
function formatSimpleValue(value) {
  var basic = formatIdentityFreeValue(value);
  if (basic) return basic;
  try {
    return String(value);
  } catch (err) {
    if (err.name === 'TypeError') {
      return Object.prototype.toString.call(value);
    }
    throw err;
  }
}
"#;
