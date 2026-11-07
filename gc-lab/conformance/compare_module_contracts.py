#!/usr/bin/env python3
"""Link/evaluate the exact module graphs from Rust contracts in Node's module VM."""
import argparse, hashlib, json, re, subprocess
from pathlib import Path
from run import REVISION
HERE = Path(__file__).resolve().parent
STRING = r'"(?:[^"\\]|\\.)*"'
NODE = r'''
const fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const input=JSON.parse(fs.readFileSync(0,'utf8'));
const harness=['sta.js','assert.js'].map(n=>fs.readFileSync(path.join(input.source,'harness',n),'utf8')).join('\n');
(async()=>{
 const failures=[];let passed=0;
 for(const test of input.cases){
  try{
   const context=vm.createContext({});vm.runInContext(harness,context);
   const modules=new Map(test.modules.map(m=>[m.url,new vm.SourceTextModule(m.source,{context,identifier:m.url})]));
   const resolutions=new Map(test.modules.map(m=>[m.url,new Map(m.resolutions)]));
   let stage='link',error;
   try{
    await modules.get('main').link((specifier,ref)=>{
     const target=resolutions.get(ref.identifier).get(specifier);if(!modules.has(target))throw Error('missing module '+target);return modules.get(target);
    });
    stage='evaluate';await modules.get('main').evaluate({timeout:1000});
   }catch(e){error=e;}
   await new Promise(r=>setImmediate(r));
   if(test.link_error){if(stage!=='link'||!error||error.constructor.name!=='SyntaxError')throw Error('expected link SyntaxError');}
   else if(test.error){if(stage!=='evaluate'||!error||error.constructor.name+': '+error.message!==test.error)throw Error('wrong evaluation error: '+error);}
   else if(error){throw error;}
   passed++;
  }catch(error){failures.push({name:test.name,error:String(error)});}
 }
 console.log(JSON.stringify({node_version:process.version,total:input.cases.length,passed,failures},null,2));
})().catch(e=>{console.error(e);process.exitCode=1;});
'''
def main():
    p=argparse.ArgumentParser(__doc__);p.add_argument('--source',type=Path,required=True);p.add_argument('--output',type=Path,default=HERE/'module-contract-node.json');a=p.parse_args()
    assert (a.source/'.revision').read_text().strip()==REVISION
    contracts=HERE.parent/'tests/modules.rs';text=contracts.read_text();cases=[]
    for block in text.split('#[test]')[1:]:
        name=re.search(r'fn (\w+)\(',block).group(1);modules=[]
        for match in re.finditer(r'\bsource\(\s*('+STRING+r')\s*,\s*('+STRING+r')\s*,\s*&\[(.*?)\]\s*,?\s*\)',block,re.S):
            url,source=map(json.loads,match.group(1,2));resolutions=[list(map(json.loads,m))for m in re.findall(r'\(\s*('+STRING+r')\s*,\s*('+STRING+r')\s*\)',match.group(3))]
            modules.append(dict(url=url,source=source,resolutions=resolutions))
        if modules:
            error=re.search(r'ModuleStatus::Errored\(\s*('+STRING+')',block)
            cases.append(dict(name=name,modules=modules,link_error='missing_exports' in name,error=json.loads(error.group(1))if error else None))
    assert cases
    run=subprocess.run(['node','--experimental-vm-modules','-e',NODE],input=json.dumps(dict(source=str(a.source.resolve()),cases=cases)),capture_output=True,text=True,check=True,timeout=60)
    report=json.loads(run.stdout);report.update(revision=REVISION,contracts_sha256=hashlib.sha256(contracts.read_bytes()).hexdigest(),runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),cases=[dict(name=c['name'],sources={m['url']:hashlib.sha256(m['source'].encode()).hexdigest()for m in c['modules']})for c in cases])
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:report[k]for k in ('node_version','total','passed','failures')},indent=2));return bool(report['failures'])
if __name__=='__main__':raise SystemExit(main())
