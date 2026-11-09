//! One Test262 script case per process (strict unless `--sloppy`); no source rewriting or helper stubs.
//! `--include=FILE` prepends an official harness include (as the reference runner does).
//! `--gc-stress` collects at every allocation (rooting debug); the default is the threshold policy.
use gc_lab::{
    script::{parser, Engine, GcPolicy, ScriptError},
    Model,
};
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
    let negative = remaining.into_iter().find(|s| !s.starts_with("--"));
    let source = std::fs::read_to_string(path).expect("source");
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
        for &strict in modes {
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
    e.set_gc_policy(if gc_stress {
        GcPolicy::Stress
    } else {
        GcPolicy::Threshold
    });
    // The engine default (10k steps) is an embedding safety budget; conformance cases such as
    // testTypedArray.js loops need far more. The runner's per-case timeout bounds runaway cases.
    e.set_step_limit(50_000_000);
    let prefix = if sloppy { "" } else { "\"use strict\";\n" };
    let full = format!("{prefix}{ASSERT_GLOBALS}{includes}{source}");
    let mut result = e.execute(&full);
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
            (Err(ScriptError::Syntax(s)), _) => ("unsupported", format!("syntax: {}", s.message)),
            (Err(ScriptError::Unsupported(s)), _) => ("unsupported", s.into()),
            (Err(ScriptError::Thrown(ex)), Some(expected))
                if ex.name.as_deref() == Some(expected.as_str()) =>
            {
                ("passed", String::new())
            }
            (Ok(()), None) => ("passed", String::new()),
            (result, expected) => ("failed", format!("{result:?}; expected {expected:?}")),
        }
    };
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
"#;
