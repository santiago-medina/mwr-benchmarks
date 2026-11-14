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

FULL_SCOPES = ['built-ins', 'language']  # whole-suite yardstick for replacing SpiderMonkey
TEMPORAL_SCOPES = ['built-ins/Temporal']  # needs the gc-lab/temporal build (--binary)
ASYNC_SCOPES = ['language/expressions/async-function', 'language/expressions/async-arrow-function', 'language/expressions/await', 'language/statements/async-function']

# Negative parse cases for syntax the parser does not implement could only pass by accident.
UNIMPLEMENTED_SYNTAX = {'decorators'}
def field(meta, name):
    match = re.search(r'^' + name + r':\s*([^\n]*)(?:\n((?:[ \t]+[^\n]*\n)*))?', meta, re.M)
    return (match.group(1) + '\n' + (match.group(2) or '')).strip() if match else ''
def items(value):
    return re.findall(r'[\w.$/-]+', value)
def main():
    p=argparse.ArgumentParser(__doc__)
    p.add_argument('--workers',type=int,default=4)
    p.add_argument('--engine',choices=['tree','vm'],default=None,help='explicit JS evaluator; default keeps checker setting')
    p.add_argument('--gc-stress',action='store_true',help='force a GC at every allocation boundary')
    p.add_argument('--paths-file',type=Path,default=None,help='JSON paths list or full summary.json failures')
    p.add_argument('--profile',choices=['objects','arrays','promises','async','temporal','full'],default='objects')
    p.add_argument('--source',type=Path,required=True)
    p.add_argument('--binary',type=Path,default=HERE.parent/'target/release/js_check')
    p.add_argument('--output',type=Path,default=None)
    p.add_argument('--record-baseline',action='store_true')
    p.add_argument('--retry-timeouts',type=int,default=0,metavar='W',help='rerun runner timeouts afterwards with W workers and the same 10 s budget (load noise, not a larger budget)')
    p.add_argument('--check-baseline',action='store_true',help='gate on lost passes and newly failing cases; known failures stay reported')
    args=p.parse_args(); binary=args.binary.resolve(); root=args.source.resolve()/'test'
    assert (args.source/'.revision').read_text().strip()==REVISION, 'Use the pinned verified source from fetch.py'
    scopes={'objects':SCOPES,'arrays':ARRAY_SCOPES,'promises':PROMISE_SCOPES,'async':ASYNC_SCOPES,'temporal':TEMPORAL_SCOPES,'full':FULL_SCOPES}[args.profile]
    suffix='' if args.profile=='objects' else '-'+args.profile
    args.output=args.output or HERE/('results'+suffix)
    cases=sorted({path for scope in scopes for path in (root.glob(scope) if '*' in scope else (root/scope).rglob('*.js')) if not path.name.endswith('_FIXTURE.js')})
    assert all((root/(scope.split('/*')[0])).is_dir() for scope in scopes), 'Missing scope'
    if args.paths_file:
        selection=json.loads(args.paths_file.read_text())
        if isinstance(selection,dict):
            selection=[item['path'] for item in selection['failures']]
        assert isinstance(selection,list) and all(isinstance(p,str) for p in selection), 'Invalid path list'
        selected=set(selection)
        available={path.relative_to(root).as_posix() for path in cases}
        assert selected and selected <= available, 'Missing requested cases: '+str(sorted(selected-available)[:5])
        cases=[path for path in cases if path.relative_to(root).as_posix() in selected]
    def run(path):
        source=path.read_text();meta=re.search(r'/\*---(.*?)---\*/',source,re.S)
        assert meta, path
        meta=meta.group(1); flags=items(field(meta,'flags')); includes=items(field(meta,'includes')); negative=field(meta,'negative')
        result=dict(path=path.relative_to(root).as_posix(),source_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
        reason=None
        if ('module' in flags and args.profile!='full') or ('async' in flags and args.profile not in ('promises','async','temporal','full')): reason='unsupported execution flags: '+','.join(flags)
        elif includes and args.profile not in ('temporal','full'): reason='unsupported harness includes: '+','.join(includes)
        elif negative and not re.search(r'phase:\s*(runtime|parse|resolution)',negative): reason='negative test with unknown phase'
        elif negative and re.search(r'phase:\s*resolution',negative) and 'module' not in flags: reason='negative resolution test outside a module'
        elif negative and 'phase: parse' in negative and args.profile not in ('temporal','full'): reason='negative parse test outside full profile'
        elif negative and set(items(field(meta,'features')))&UNIMPLEMENTED_SYNTAX: reason='negative parse test for unimplemented syntax: '+','.join(sorted(set(items(field(meta,'features')))&UNIMPLEMENTED_SYNTAX))
        if reason: return dict(result,status='unsupported',reason=reason,assertions=0)
        expected=re.search(r'type:\s*(\w+)',negative) if negative else None
        parse_negative=bool(negative) and re.search(r'phase:\s*parse',negative) is not None
        resolution_negative=bool(negative) and re.search(r'phase:\s*resolution',negative) is not None
        command=[str(binary),str(path)] + ([f'--engine={args.engine}'] if args.engine else []) + (['--gc-stress'] if args.gc_stress else []) + ([expected.group(1)] if expected else []) + (['--negative-parse'] if parse_negative else []) + (['--negative-resolution'] if resolution_negative else []) + (['--module'] if 'module' in flags else []) + (['--strict-only'] if parse_negative and 'onlyStrict' in flags else []) + (['--async'] if 'async' in flags else []) + (['--can-block'] if 'CanBlockIsFalse' not in flags else []) + (['--sloppy'] if 'noStrict' in flags or 'raw' in flags else []) + (['--include='+str(root.parent/'harness'/i) for i in includes] if args.profile in ('temporal','full') and 'raw' not in flags else [])
        try:
            r=subprocess.run(command,capture_output=True,text=True,timeout=10)
            data=json.loads(r.stdout)
            assert r.returncode==dict(passed=0,failed=1,unsupported=2)[data['status']], (r.returncode,data)
            return dict(result,**data)
        except (subprocess.TimeoutExpired,ValueError,AssertionError) as e:
            return dict(result,status='failed',reason='runner: '+str(e),assertions=0)
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool: results=list(pool.map(run,cases))
    retry=dict(workers=args.retry_timeouts,timeouts=0,passed=0)
    if args.retry_timeouts:
        # A timeout under full load can be load noise: rerun those cases with fewer workers, same budget.
        slow=[i for i,r in enumerate(results) if r['status']=='failed' and r['reason'].startswith('runner: Command') and 'timed out' in r['reason']]
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.retry_timeouts) as pool: again=list(pool.map(run,[root/results[i]['path'] for i in slow]))
        for i,r in zip(slow,again): results[i]=dict(r,retried=True)
        retry.update(timeouts=len(slow),passed=sum(r['status']=='passed' for r in again))
    counts=dict(collections.Counter(r['status'] for r in results)); passing=sorted(r['path'] for r in results if r['status']=='passed')
    baseline=HERE/('required-pass'+suffix+'.json'); required=json.loads(baseline.read_text()) if baseline.exists() else []
    regressions=sorted(set(required)-set(passing))
    known_file=HERE/('known-failures'+suffix+'.json')
    known=json.loads(known_file.read_text()) if known_file.exists() else []
    new_failures=sorted(r['path'] for r in results if r['status']=='failed' and r['path'] not in known)
    implementation=hashlib.sha256()
    for path in sorted((HERE.parent/'src').rglob('*.rs')):
        implementation.update(path.relative_to(HERE.parent).as_posix().encode()+b'\0'+path.read_bytes())
    summary=dict(revision=REVISION,archive_sha256='01b5c93c8960a549375fd9d8085d06aadcc96cd29cf6bc137eff32ac98d592eb',implementation_sha256=implementation.hexdigest(),runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),new_failures=new_failures,variant='scripts: strict prefix, or sloppy for noStrict/raw; modules (full profile): includes as scripts, then the module record; original test body',profile=args.profile,retry_timeouts=retry,scopes=scopes,total=len(results),counts=counts,assertions=sum(r['assertions'] for r in results),regressions=regressions,unsupported_reasons=dict(collections.Counter(r['reason'] for r in results if r['status']=='unsupported')),failures=[r for r in results if r['status']=='failed'])
    summary['audit_config']=dict(engine=args.engine or 'checker-default',gc_stress=args.gc_stress,paths_file=str(args.paths_file) if args.paths_file else None)
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
