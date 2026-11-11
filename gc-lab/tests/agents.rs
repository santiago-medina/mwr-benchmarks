//! test262 agents (AGENTS.md): one thread and one engine per agent, sharing only
//! SharedArrayBuffer blocks.
use gc_lab::{script::Engine, Model};

fn main_agent() -> Engine<'static> {
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.enable_test_harness();
    e.set_can_block(true);
    e.set_step_limit(50_000_000);
    e
}

const REPORT: &str = r#"
function report() {
  let r;
  while ((r = $262.agent.getReport()) === null) $262.agent.sleep(1);
  return r;
}
"#;

#[test]
fn sync_wait_is_woken_by_another_agent() {
    let mut e = main_agent();
    let script = format!(
        r#"{REPORT}
$262.agent.start(`
  $262.agent.receiveBroadcast(function (sab) {{
    const i32 = new Int32Array(sab);
    Atomics.add(i32, 1, 1);
    $262.agent.report(Atomics.wait(i32, 0, 0, 10000));
    $262.agent.leaving();
  }});
`);
const i32 = new Int32Array(new SharedArrayBuffer(8));
$262.agent.broadcast(i32.buffer);
while (Atomics.load(i32, 1) !== 1) ;
let woken = 0;
while (woken === 0) {{ woken = Atomics.notify(i32, 0); }}
const r = report();
if (r !== "ok" || woken !== 1) throw new Error(r + " " + woken);
"#
    );
    e.execute(&script).unwrap();
    e.release_globals();
}

#[test]
fn waiters_of_a_dead_agent_are_purged() {
    // The agent leaves a pending waitAsync and dies; notify must not count its waiter.
    let mut e = main_agent();
    let script = format!(
        r#"{REPORT}
$262.agent.start(`
  $262.agent.receiveBroadcast(function (sab) {{
    const i32 = new Int32Array(sab);
    Atomics.waitAsync(i32, 0, 0);
    $262.agent.report("waiting");
    throw new Error("agent ends");
  }});
`);
const i32 = new Int32Array(new SharedArrayBuffer(4));
$262.agent.broadcast(i32.buffer);
if (report() !== "waiting") throw new Error("no report");
let failed = false;
try {{ report(); }} catch (err) {{ failed = /agent ends/.test(String(err)); }}
if (!failed) throw new Error("agent error not surfaced");
const n = Atomics.notify(i32, 0);
if (n !== 0) throw new Error("notified " + n + " waiters of a dead agent");
"#
    );
    e.execute(&script).unwrap();
    e.release_globals();
}
