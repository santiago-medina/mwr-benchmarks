#!/usr/bin/env python3
"""Differential Promise combinator iterator-close probes against Node 24.

Diagnostics may show MWR failures: only the probe runner and native reference
must succeed. Outputs exact path/mode/error for reproducing semantic defects.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve()
results = []

templates = {
    "resolve-throw-close": """
        let closed=0;
        const iterable={ [Symbol.iterator]() { return {
            next() {return {value: 1, done: false};},
            return() {closed++;return {};}
        };}};
        const orig=Promise.resolve;
        Promise.resolve=function() {throw Error('reject resolve');};
        const p=Promise.METHOD(iterable);
        Promise.resolve=orig;
        p.catch(()=>{});
        if(closed!==1)throw Error('close count: '+closed+' expected 1');
    """,
    "then-getter-close": """
        let closed=0;
        const iterable={ [Symbol.iterator]() {return {
            next() {return {value: 1, done: false};},
            return() {closed++;return {};}
        };}};
        const orig=Promise.resolve;
        Promise.resolve=function(){return {get then(){throw Error('then getter');}};};
        const p=Promise.METHOD(iterable);
        Promise.resolve=orig;
        p.catch(()=>{});
        if(closed!==1)throw Error('close count: '+closed+' expected 1');
    """,
    "then-call-close": """
        let closed=0;
        const iterable={ [Symbol.iterator]() {return {
            next() {return {value: 1,done: false};},
            return() {closed++;return {};}
        };}};
        const orig=Promise.resolve;
        Promise.resolve=function(){return {then(){throw Error('then call');}};};
        const p=Promise.METHOD(iterable);
        Promise.resolve=orig;
        p.catch(()=>{});
        if(closed!==1)throw Error('close count: '+closed+' expected 1');
    """,
    "native-promise-then-call-close": """
        let closed=0;
        const pending=new Promise(function(){});
        Object.defineProperty(pending,'then',{value:function(){throw Error('then call');}});
        const iterable={ [Symbol.iterator]() {return {
            next() {return {value:pending,done:false};},
            return() {closed++;return {};}
        };}};
        const p=Promise.METHOD(iterable);
        p.catch(()=>{});
        if(closed!==1)throw Error('close count: '+closed+' expected 1');
    """,
    "native-promise-then-getter-close": """
        let closed=0;
        const pending=new Promise(function(){});
        Object.defineProperty(pending,'then',{get:function(){throw Error('then getter');}});
        const iterable={ [Symbol.iterator]() {return {
            next() {return {value:pending,done:false};},
            return() {closed++;return {};}
        };}};
        const p=Promise.METHOD(iterable);
        p.catch(()=>{});
        if(closed!==1)throw Error('close count: '+closed+' expected 1');
    """,
    "resolve-throw-nonobject-return": """
        let closed=0;
        const iterable={ [Symbol.iterator]() { return {
            next() {return {value:1,done:false};},
            return() {closed++;}
        };}};
        const orig=Promise.resolve;
        Promise.resolve=function(){throw Error('reject resolve');};
        const p=Promise.METHOD(iterable);
        Promise.resolve=orig;
        p.catch(()=>{});
        if(closed!==1)throw Error('close count: '+closed+' expected 1');
    """,
    "step-value-no-close": """
        let closed=0;
        const iterable={ [Symbol.iterator]() {return {
            next() {return {done:false,get value(){throw Error('step value');}};},
            return() {closed++;return {};}
        };}};
        const p=Promise.METHOD(iterable);
        p.catch(()=>{});
        if(closed!==0)throw Error('close count: '+closed+' expected 0');
    """,
    "step-done-no-close": """
        let closed=0;
        const iterable={ [Symbol.iterator]() {return {
            next() {return {get done(){throw Error('step done');}};},
            return() {closed++;return {};}
        };}};
        const p=Promise.METHOD(iterable);
        p.catch(()=>{});
        if(closed!==0)throw Error('close count: '+closed+' expected 0');
    """,
}
with tempfile.TemporaryDirectory() as folder:
    for method in ("all", "allSettled", "race"):
        for name, source in templates.items():
            case = Path(folder) / (method + "-" + name + ".js")
            case.write_text(source.replace("METHOD", method))
            native = subprocess.run(["node", str(case)], capture_output=True, text=True, timeout=30)
            results.append(dict(case=case.name, runner="node-24", code=native.returncode,
                                reason=(native.stderr or native.stdout)[-500:]))
            for engine in ("tree", "vm"):
                for stress in (False, True):
                    cmd = [str(binary), str(case), "--engine=" + engine]
                    if stress:
                        cmd.append("--gc-stress")
                    try:
                        p = subprocess.run(cmd, capture_output=True, text=True, timeout=40)
                        data = json.loads(p.stdout)
                        results.append(dict(case=case.name, runner=engine,
                                            gc_stress=stress, code=p.returncode,
                                            status=data.get("status"), reason=data.get("reason", ""),
                                            cleanup=data.get("cleanup")))
                    except Exception as exc:
                        results.append(dict(case=case.name, runner=engine,
                                            gc_stress=stress, code=None,
                                            status="runner_error", reason=repr(exc)))
out = Path("promise-iterator-close-diagnostics.json")
out.write_text(json.dumps(results, indent=2) + "\n")
counts = {}
for x in results:
    key = x.get("runner", "unknown")
    passed = (x.get("code") == 0 and (key == "node-24" or
              (x.get("status") == "passed" and x.get("cleanup") is True)))
    counts.setdefault(key, [0, 0])[int(not passed)] += 1
print(json.dumps({"total":len(results),
                  "counts":{k:{"passed":v[0],"failed":v[1]} for k,v in counts.items()},
                  "mwr_failures":[x for x in results if x.get("runner") != "node-24"
                                  and x.get("status") != "passed"]}, indent=2))
assert len(results) == 120
assert all(x["code"] == 0 for x in results if x["runner"] == "node-24"), \
    "Native reference probes failed; review diagnostic assumptions"
