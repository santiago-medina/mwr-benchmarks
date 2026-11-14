#!/usr/bin/env python3
"""Small deterministic VM/tree/GC-stress cross-mode smoke; not a Test262 substitute."""
import itertools
import json
import os
import pathlib
import subprocess
import sys
import tempfile

binary = pathlib.Path(sys.argv[1]).resolve()
cases = {
    "closures": """let n=2; function make(){let x=3; return function(){x+=1; return x+n;};} let f=make(); if(f()!==6||f()!==7) throw Error('closure');""",
    "arrays": """let a=[1,2,3,4]; let b=a.map(x=>x*2); if(b.join(',')!=='2,4,6,8')throw Error('map'); a.splice(1,1); if(a.length!==3||a[1]!==3)throw Error('splice');""",
    "iterator": """let s=0; for(const n of [1,2,3,4]) s+=n; if(s!==10)throw Error('iterator');""",
    "property": """let x={a:1,get b(){return this.a+1;},set b(v){this.a=v-1;}}; x.b=8; if(x.a!==7||x.b!==8)throw Error('getter');""",
    "exception": """function f(){try{throw 4;}finally{globalThis.marker=9;}} let got=0; try{f();}catch(x){got=x;} if(got!==4||marker!==9)throw Error('finally');""",
    "promise": """let n=0; Promise.resolve(2).then(x=>{n=x; if(n!==2)throw Error('promise');});""",
    "cycle": """let a={}; let b={}; a.other=b; b.other=a; a=null; b=null;""",
    "atomics": """let s=new SharedArrayBuffer(8); let a=new Int32Array(s); if(Atomics.add(a,0,3)!==0||Atomics.load(a,0)!==3)throw Error('atomics');""",
}
out = []
with tempfile.TemporaryDirectory() as d:
    for name, source in cases.items():
        path = pathlib.Path(d, name + ".js")
        path.write_text(source)
        for mode, stress in itertools.product(("tree", "vm"), (False, True)):
            cmd = [str(binary), str(path), "--engine=" + mode]
            if stress:
                cmd.append("--gc-stress")
            try:
                proc = subprocess.run(cmd, text=True, capture_output=True, timeout=40)
                last = proc.stdout.strip().splitlines()[-1]
                data = json.loads(last)
                status = data["status"]
                ok = proc.returncode == 0 and status == "passed" and data.get("cleanup") is True
                detail = data.get("reason", "")
            except Exception as exc:
                ok = False
                detail = repr(exc)
                status = "runner_error"
            out.append(dict(case=name, mode=mode, gc_stress=stress, status=status, ok=ok, detail=detail))
print(json.dumps({"total": len(out), "passed": sum(x["ok"] for x in out), "failures": [x for x in out if not x["ok"]]}, indent=2))
sys.exit(0 if all(x["ok"] for x in out) else 1)
