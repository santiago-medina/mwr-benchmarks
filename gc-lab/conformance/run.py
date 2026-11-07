#!/usr/bin/env python3
"""Run every test in pinned ordinary-object directories; strict variant only.
Unsupported grammar/harness/standard intrinsics remain visible in the denominator.
"""
import argparse, collections, concurrent.futures, hashlib, json, re, subprocess
from pathlib import Path
HERE = Path(__file__).resolve().parent
REVISION = 'e4fbf57745c6aae3d1f12f181ecef78b1665d812'
SCOPES = ['built-ins/Object/' + n for n in ('create','defineProperty','defineProperties','getOwnPropertyDescriptor','getPrototypeOf','setPrototypeOf','preventExtensions','isExtensible','is','freeze','seal','isFrozen','isSealed')] + ['built-ins/Reflect/' + n for n in ('get','set','defineProperty','setPrototypeOf')]
ARRAY_SCOPES = ['built-ins/Array/*.js', 'built-ins/Array/isArray'] + ['built-ins/Array/prototype/'+n for n in ('push','pop','join','toString','map','filter','forEach','some','every','values','keys','entries','Symbol.iterator')] + ['built-ins/ArrayIteratorPrototype/next','language/statements/for-of','language/expressions/array']

PROMISE_SCOPES=['built-ins/Promise/*.js','built-ins/Promise/resolve','built-ins/Promise/reject','built-ins/Promise/prototype/then','built-ins/Promise/prototype/catch','built-ins/Promise/prototype/finally']

def field(meta, name):
    match = re.search(r'^' + name + r':\s*([^\n]*)(?:\n((?:[ \t]+[^\n]*\n)*))?', meta, re.M)
    return (match.group(1) + '\n' + (match.group(2) or '')).strip() if match else ''
def items(value):
    return re.findall(r'[\w.$/-]+', value)
def main():
    p=argparse.ArgumentParser(__doc__)
    p.add_argument('--profile',choices=['objects','arrays','promises'],default='objects')
    p.add_argument('--source',type=Path,required=True)
    p.add_argument('--binary',type=Path,default=HERE.parent/'target/release/js_check')
    p.add_argument('--output',type=Path,default=None)
    p.add_argument('--record-baseline',action='store_true')
    p.add_argument('--check-baseline',action='store_true',help='gate on lost passes and newly failing cases; known failures stay reported')
    args=p.parse_args(); binary=args.binary.resolve(); root=args.source.resolve()/'test'
    assert (args.source/'.revision').read_text().strip()==REVISION, 'Use the pinned verified source from fetch.py'
    scopes={'objects':SCOPES,'arrays':ARRAY_SCOPES,'promises':PROMISE_SCOPES}[args.profile]
    suffix='' if args.profile=='objects' else '-'+args.profile
    args.output=args.output or HERE/('results'+suffix)
    cases=sorted({path for scope in scopes for path in (root.glob(scope) if '*' in scope else (root/scope).rglob('*.js')) if not path.name.endswith('_FIXTURE.js')})
    assert all((root/(scope.split('/*')[0])).is_dir() for scope in scopes), 'Missing scope'
    def run(path):
        source=path.read_text();meta=re.search(r'/\*---(.*?)---\*/',source,re.S)
        assert meta, path
        meta=meta.group(1); flags=items(field(meta,'flags')); includes=items(field(meta,'includes')); negative=field(meta,'negative')
        result=dict(path=path.relative_to(root).as_posix(),source_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
        reason=None
        if any(f in flags for f in ('noStrict','module','raw')) or ('async' in flags and args.profile!='promises'): reason='unsupported execution flags: '+','.join(flags)
        elif includes: reason='unsupported harness includes: '+','.join(includes)
        elif negative and not re.search(r'phase:\s*runtime',negative): reason='negative parse/resolution test outside runtime profile'
        if reason: return dict(result,status='unsupported',reason=reason,assertions=0)
        expected=re.search(r'type:\s*(\w+)',negative) if negative else None
        command=[str(binary),str(path)] + ([expected.group(1)] if expected else []) + (['--async'] if 'async' in flags else [])
        try:
            r=subprocess.run(command,capture_output=True,text=True,timeout=10)
            data=json.loads(r.stdout)
            assert r.returncode==dict(passed=0,failed=1,unsupported=2)[data['status']], (r.returncode,data)
            return dict(result,**data)
        except (subprocess.TimeoutExpired,ValueError,AssertionError) as e:
            return dict(result,status='failed',reason='runner: '+str(e),assertions=0)
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool: results=list(pool.map(run,cases))
    counts=dict(collections.Counter(r['status'] for r in results)); passing=sorted(r['path'] for r in results if r['status']=='passed')
    baseline=HERE/('required-pass'+suffix+'.json'); required=json.loads(baseline.read_text()) if baseline.exists() else []
    regressions=sorted(set(required)-set(passing))
    known_file=HERE/('known-failures'+suffix+'.json')
    known=json.loads(known_file.read_text()) if known_file.exists() else []
    new_failures=sorted(r['path'] for r in results if r['status']=='failed' and r['path'] not in known)
    implementation=hashlib.sha256()
    for path in sorted((HERE.parent/'src').rglob('*.rs')):
        implementation.update(path.relative_to(HERE.parent).as_posix().encode()+b'\0'+path.read_bytes())
    summary=dict(revision=REVISION,archive_sha256='01b5c93c8960a549375fd9d8085d06aadcc96cd29cf6bc137eff32ac98d592eb',implementation_sha256=implementation.hexdigest(),runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),new_failures=new_failures,variant='strict script only; standard strict prefix; original test body',profile=args.profile,scopes=scopes,total=len(results),counts=counts,assertions=sum(r['assertions'] for r in results),regressions=regressions,unsupported_reasons=dict(collections.Counter(r['reason'] for r in results if r['status']=='unsupported')),failures=[r for r in results if r['status']=='failed'])
    args.output.mkdir(parents=True,exist_ok=True)
    (args.output/'cases.json').write_text(json.dumps(results,indent=2)+'\n')
    (args.output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    if args.record_baseline:
        baseline.write_text(json.dumps(passing,indent=2)+'\n')
        known_file.write_text(json.dumps(sorted(r['path'] for r in results if r['status']=='failed'),indent=2)+'\n')
    print(json.dumps({k:summary[k] for k in ('revision','total','counts','assertions','regressions')},indent=2))
    if args.check_baseline:
        assert required and known_file.exists(), 'A reviewed baseline is required'
        return bool(regressions or new_failures)
    return bool(regressions or counts.get('failed',0))
if __name__=='__main__': raise SystemExit(main())
