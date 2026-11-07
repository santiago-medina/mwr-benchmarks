#!/usr/bin/env python3
"""Real module HTTP graphs, script policies, top-level await and retired-source GC."""
import argparse,hashlib,http.server,json,os,subprocess,tempfile,threading
from pathlib import Path
HERE=Path(__file__).resolve().parent

def main():
    p=argparse.ArgumentParser(__doc__);p.add_argument('--servo',type=Path,required=True);p.add_argument('--output',type=Path,default=HERE.parent/'native-module-results');a=p.parse_args()
    binary=a.servo.resolve();out=a.output.resolve();out.mkdir(parents=True,exist_ok=True);results={}
    for case in ['success','external-entry','connect-blocked','script-blocked','cors','mime','http-error','link-error','evaluation-error','navigation','disabled']:
        release=threading.Event();old_image=threading.Event();requests=[]
        scripts={
            '/main.js':"import {label} from './format.js';import {count,bump} from './counter.js';import {text} from './remote.js';import {cycle} from './a.js';bump();const result=document.querySelector('#result');result.textContent=label()+' | '+String(count)+' | '+text+' | '+cycle+' | '+String(globalThis.sourceRuns);",
            '/shared.js':"if(globalThis.sourceRuns===undefined){globalThis.sourceRuns=0;}globalThis.sourceRuns=globalThis.sourceRuns+1;export const tag='modular';",
            '/format.js':"import {tag} from './shared.js';export function label(){return tag;}",
            '/counter.js':"import {tag} from './shared.js';export let count=0;export function bump(){count=count+1;}",
            '/remote.js':"const response=await fetch('/payload');export const text=await response.text();",
            '/a.js':"import {read} from './b.js';export function ping(){return 'ciclo';}export const cycle=read();",
            '/b.js':"import {ping} from './a.js';export function read(){return ping();}",
            '/slow.js':"document.querySelector('#result').textContent='retired source executed';",
        }
        if case=='connect-blocked':scripts['/remote.js']="export const text=await Promise.resolve('sin red');"
        if case=='link-error':scripts['/main.js']="import {absent} from './shared.js';document.querySelector('#result').textContent='bad';"
        if case=='evaluation-error':scripts['/main.js']="await 1;throw new Error('module failure');"

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_):pass
            def do_GET(self):
                requests.append(self.path)
                status=200;content_type='application/javascript; charset=utf-8'
                if self.path=='/':data=fixture.encode();content_type='text/html; charset=utf-8'
                elif self.path.startswith('/?after=1'):
                    data=b'<meta charset="utf-8"><h1>Modulo anterior recogido</h1><img src="/hold"><script type="application/x-mwr-js">const ready=1;</script>';content_type='text/html'
                elif self.path in ('/hold','/hold?old'):
                    (old_image if self.path.endswith('?old') else release).wait(45)
                    data=b'<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>';content_type='image/svg+xml'
                elif self.path=='/payload':data=b'respuesta HTTP';content_type='text/plain'
                elif self.path in scripts:
                    data=scripts[self.path].encode()
                    if case=='mime' and self.path=='/remote.js':content_type='text/plain'
                    if case=='http-error' and self.path=='/remote.js':status=404
                else:status=404;data=b'not found'
                self.send_response(status);self.send_header('Content-Type',content_type);self.send_header('Content-Length',str(len(data)));self.end_headers()
                if self.path=='/slow.js':self.wfile.flush();release.wait(45)
                try:self.wfile.write(data)
                except (BrokenPipeError,ConnectionResetError):pass

        servers=[http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler) for _ in range(2)]
        for server in servers:threading.Thread(target=server.serve_forever,daemon=True).start()
        if case=='cors':scripts['/main.js']=f"import 'http://127.0.0.1:{servers[1].server_port}/remote.js';"
        policy={
            'connect-blocked':"script-src 'self' 'unsafe-inline'; connect-src 'none'",
            'script-blocked':"script-src 'unsafe-inline'; connect-src *",
        }.get(case,'')
        csp='<meta http-equiv="Content-Security-Policy" content="'+policy+'">'if policy else ''
        script='<script type="application/x-mwr-module" src="/main.js"></script>'if case=='external-entry'else '<script type="application/x-mwr-module" src="/slow.js"></script>'if case=='navigation'else '<script type="application/x-mwr-module">import "./main.js";</script>'
        fixture='<meta charset="utf-8">'+csp+'<title>Modulos ES en Rust</title><style>body{font:24px sans-serif;padding:48px;background:#edf4f2;color:#18362d}#result{background:white;padding:24px}</style><h1>Modulos, ciclos y red nativa</h1><p id="result">Pendiente</p>'+(''if case=='disabled'else'<img src="/hold?old" alt="">')+script
        try:
            with tempfile.TemporaryDirectory(prefix='mwr-modules-')as temp:
                env=dict(os.environ,MWR_NATIVE_JS='1',MWR_NATIVE_MICROTASK_GC_STRESS='1',LIBGL_ALWAYS_SOFTWARE='1',EGL_PLATFORM='surfaceless')
                for key in ('MWR_NATIVE_SELF_TEST','MWR_NATIVE_GC_TEST','MWR_NATIVE_FETCH_TEST','MWR_NATIVE_FETCH_NAVIGATION','MWR_NATIVE_MODULE_NAVIGATION'):env.pop(key,None)
                if case=='disabled':env.pop('MWR_NATIVE_JS',None)
                if case=='navigation':env.update(MWR_NATIVE_MODULE_NAVIGATION='1',MWR_NATIVE_FETCH_NAVIGATION='1')
                for key in ('XDG_RUNTIME_DIR','XDG_CACHE_HOME','XDG_CONFIG_HOME','TMPDIR'):
                    directory=Path(temp)/key;directory.mkdir(mode=0o700);env[key]=str(directory)
                image=out/(case+'.png');lines=[]
                process=subprocess.Popen([str(binary),'--headless','--window-size=1000x650','--temporary-storage','-x','-o',str(image),f'http://127.0.0.1:{servers[0].server_port}/'],env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
                def read():
                    for line in process.stdout:
                        lines.append(line)
                        if line.startswith('MWR_NATIVE_FETCH_NAVIGATING'):old_image.set()
                        if line.startswith(('MWR_NATIVE_MODULE_PROOF ','MWR_NATIVE_FETCH_PROOF ')):old_image.set();release.set()
                reader=threading.Thread(target=read,daemon=True);reader.start()
                try:code=process.wait(timeout=70)
                finally:
                    if process.poll()is None:process.kill();process.wait()
                    reader.join(timeout=5);log=''.join(lines);(out/(case+'.log')).write_text(log);(out/(case+'.html')).write_text(fixture)
                assert code==0,(case,code,log[-6000:])
                assert not any(marker in log for marker in ('MWR_NATIVE_ERROR ','MWR_NATIVE_MODULE_ERROR ','MWR_NATIVE_MICROTASK_ERROR ','MWR_NATIVE_UNHANDLED_REJECTION ')),(case,log)
                proofs=[json.loads(line.removeprefix('MWR_NATIVE_MODULE_PROOF '))for line in lines if line.startswith('MWR_NATIVE_MODULE_PROOF ')]
                if case=='navigation':
                    navigation=[json.loads(line.removeprefix('MWR_NATIVE_FETCH_PROOF '))for line in lines if line.startswith('MWR_NATIVE_FETCH_PROOF ')]
                    assert len(navigation)==1 and all(navigation[0][key]for key in ('native_modules','old_session_collected','old_document_collected','retired_completion_ignored')),(case,log)
                    assert 'MWR_NATIVE_STALE_MODULE_SOURCE' in log and not proofs,(case,log)
                    proof=navigation[0]
                elif case=='disabled':assert not proofs and not any(path.endswith('.js')for path in requests),(case,log);proof={"disabled":True}
                else:
                    assert len(proofs)==1,(case,log);proof=proofs[0]
                    if case in ('success','external-entry','connect-blocked'):
                        expected='modular | 1 | '+('sin red'if case=='connect-blocked'else'respuesta HTTP')+' | ciclo | 1'
                        assert proof['status']=='Evaluated' and proof['text']==expected,(case,proof)
                        assert proof['modules']==(7 if case=='external-entry'else 8),(case,proof)
                        assert requests.count('/shared.js')==1,(case,requests)
                        if case=='connect-blocked':assert '/payload'not in requests
                    else:assert proof['status'].startswith('Errored(')and proof['text']=='Pendiente',(case,proof)
                    if case=='script-blocked':assert '/main.js'not in requests,requests
                assert image.exists()and image.stat().st_size>1000
                results[case]=dict(proof=proof,requests=requests,microtask_collections=log.count('MWR_NATIVE_MICROTASK_GC\n'))
                print(case,'passed',flush=True)
        finally:
            old_image.set();release.set()
            for server in servers:server.shutdown();server.server_close()
    report=dict(tested_commit=os.environ.get('GITHUB_SHA'),servo_revision='aac43a3f31a259f04a574f5ec4e959c943ad7cc7',binary_sha256=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),sources={name:hashlib.file_digest((HERE/name).open('rb'),'sha256').hexdigest()for name in ('mwr.rs','mwr_modules.rs','mwr_network.rs','servo.patch','run_modules.py')},scenarios=results,javascript_transport=False,webdriver=False)
    (out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
