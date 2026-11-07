#!/usr/bin/env python3
"""Verify our recorded passing cases with Node and the unmodified official harness."""
import argparse,json,re,subprocess
from pathlib import Path
from run import REVISION,field
HERE=Path(__file__).resolve().parent
NODE=r'''
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const input=JSON.parse(fs.readFileSync(0,'utf8'));
const harness=['sta.js','assert.js'].map(n=>fs.readFileSync(path.join(input.source,'harness',n),'utf8')).join('\n');
const failures=[];let passed=0;
for(const entry of input.cases){
  const context=vm.createContext({});
  try {
    vm.runInContext(harness,context,{timeout:1000});
    let error;
    try {vm.runInContext('"use strict";\n'+fs.readFileSync(path.join(input.source,'test',entry.path),'utf8'),context,{timeout:1000,filename:entry.path});}
    catch(e){error=e;}
    if(entry.negative){if(!error||error.constructor!==vm.runInContext(entry.negative,context)){throw Error('wrong or missing negative error: '+String(error));}}
    else if(error){throw error;}
    passed++;
  } catch(e){failures.push({path:entry.path,error:String(e)});}
}
console.log(JSON.stringify({node_version:process.version,harness:'official sta.js + assert.js, unchanged',total:input.cases.length,passed,failures},null,2));
'''
def main():
    p=argparse.ArgumentParser(__doc__);p.add_argument('--profile',choices=['objects','arrays'],default='objects');p.add_argument('--source',type=Path,required=True);p.add_argument('--output',type=Path,default=None);a=p.parse_args()
    assert (a.source/'.revision').read_text().strip()==REVISION
    suffix='' if a.profile=='objects' else '-arrays'
    a.output=a.output or HERE/('node-report'+suffix+'.json')
    paths=json.loads((HERE/('required-pass'+suffix+'.json')).read_text());cases=[]
    for path in paths:
        source=(a.source/'test'/path).read_text();meta=re.search(r'/\*---(.*?)---\*/',source,re.S).group(1);negative=field(meta,'negative');expected=re.search(r'type:\s*(\w+)',negative) if negative else None
        cases.append(dict(path=path,negative=expected.group(1) if expected else None))
    process=subprocess.run(['node','-e',NODE],input=json.dumps(dict(source=str(a.source.resolve()),cases=cases)),capture_output=True,text=True,check=True,timeout=180)
    report=json.loads(process.stdout);report['revision']=REVISION
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2));return bool(report['failures'])
if __name__=='__main__':raise SystemExit(main())
