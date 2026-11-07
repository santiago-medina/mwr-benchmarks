#!/usr/bin/env python3
"""Seeded cyclic module graphs: compare observable evaluation order against Node."""
import argparse,hashlib,json,random,subprocess,tempfile
from pathlib import Path
HERE=Path(__file__).resolve().parent
NODE=r'''
const vm=require('node:vm'),fs=require('node:fs'),path=require('node:path');
const cases=JSON.parse(fs.readFileSync(0,'utf8'));
(async()=>{const results=[];for(const c of cases){
 const context=vm.createContext({}),modules=new Map(Object.entries(c).map(([name,source])=>[name,new vm.SourceTextModule(source,{context,identifier:name})]));
 try{await modules.get('main.js').link((s,r)=>modules.get(path.posix.normalize(path.posix.join(path.posix.dirname(r.identifier),s))));await modules.get('main.js').evaluate({timeout:1000});results.push({status:'evaluated',value:modules.get('main.js').namespace.result});}
 catch(e){results.push({status:'errored',value:e.constructor.name+': '+e.message});}
}console.log(JSON.stringify({node_version:process.version,results}));})().catch(e=>{console.error(e);process.exitCode=1;});
'''
def main():
    p=argparse.ArgumentParser(__doc__);p.add_argument('--binary',type=Path,default=HERE.parent/'target/release/module_check');p.add_argument('--output',type=Path,default=HERE/'module-differential.json');p.add_argument('--cases',type=int,default=96);a=p.parse_args();cases=[]
    for seed in range(a.cases):
        rng=random.Random(seed);n=rng.randrange(2,8);sources={'trace.js':'export const log=[];'}
        for i in range(n):
            deps=[j for j in range(n)if rng.random()<0.3]
            text="import {log} from './trace.js';"+''.join(f"import './m{j}.js';"for j in deps)
            kind=rng.randrange(4)
            suspension=['','await 1;','if(false){await 1;}',f"Promise.resolve().then(function(){{log.push('q{i}');}});if(false){{await 1;}}"][kind]
            text+=f"log.push('s{i}');"+suspension+f"log.push('e{i}');"
            sources[f'm{i}.js']=text
        sources['main.js']="import {log} from './trace.js';"+''.join(f"import './m{i}.js';"for i in range(n))+"export const result=log.join(',');"
        cases.append(sources)
    node=subprocess.run(['node','--experimental-vm-modules','-e',NODE],input=json.dumps(cases),capture_output=True,text=True,check=True,timeout=60);oracle=json.loads(node.stdout);failures=[];results=[]
    with tempfile.TemporaryDirectory(prefix='mwr-module-diff-')as temp:
        root=Path(temp)
        for seed,sources in enumerate(cases):
            folder=root/str(seed);folder.mkdir()
            for name,text in sources.items():(folder/name).write_text(text)
            run=subprocess.run([str(a.binary.resolve()),str(folder),'main.js'],capture_output=True,text=True,timeout=10)
            try:
                status,value=run.stdout.rstrip('\r\n').split('\t',1);value=bytes.fromhex(value).decode()
                actual=dict(status=status,value=value)
            except (ValueError,UnicodeDecodeError):actual=dict(status='runner-error',value=run.stdout+run.stderr)
            expected=oracle['results'][seed];passed=actual==expected
            results.append(dict(seed=seed,passed=passed,sources_sha256=hashlib.sha256(json.dumps(sources,sort_keys=True).encode()).hexdigest(),result=actual))
            if not passed:failures.append(dict(seed=seed,actual=actual,expected=expected,sources=sources))
    report=dict(node_version=oracle['node_version'],total=len(cases),passed=sum(r['passed']for r in results),failures=failures,results=results,runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(dict(node_version=report['node_version'],total=report['total'],passed=report['passed'],failing_seeds=[f['seed']for f in failures]),indent=2));return bool(failures)
if __name__=='__main__':raise SystemExit(main())
