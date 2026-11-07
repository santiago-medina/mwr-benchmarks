#!/usr/bin/env python3
"""Real HTTP + native Servo Fetch; no JS evaluation, WebDriver or VM RPC."""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument('--servo', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=HERE.parent / 'native-fetch-results')
    args = parser.parse_args()
    binary = args.servo.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    results = {}
    for scenario in ['success', 'http404', 'network-error', 'cors', 'csp']:
        release_load = threading.Event()
        headers_seen = threading.Event()
        requests = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                requests.append(self.path)
                if self.path == '/drop':
                    self.close_connection = True
                    return
                if self.path == '/':
                    data = fixture.encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/html; charset=utf-8')
                elif self.path == '/hold':
                    # Images delay load, without blocking execution of the script.
                    # Release only after native observation proves final DOM state.
                    release_load.wait(45)
                    data = b'<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>'
                    self.send_response(200)
                    self.send_header('Content-Type', 'image/svg+xml')
                else:
                    data = b'body from native networking'
                    self.send_response(404 if self.path == '/missing' else 200)
                    self.send_header('Content-Type', 'text/plain')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                if self.path in ('/data', '/missing'):
                    self.wfile.flush()
                    # The body cannot arrive before the VM's headers completion.
                    headers_seen.wait(10)
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        other = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        for service in (server, other):
            threading.Thread(target=service.serve_forever, daemon=True).start()
        url = {'success': '/data', 'http404': '/missing', 'network-error': '/drop',
               'cors': f'http://127.0.0.1:{other.server_port}/data', 'csp': '/data'}[scenario]
        expected = {'success': '200|true|body from native networking',
                    'http404': '404|false|body from native networking'}.get(scenario, 'TypeError')
        policy = '<meta http-equiv="Content-Security-Policy" content="connect-src \'none\'">' if scenario == 'csp' else ''
        fixture = '''<!doctype html><meta charset="utf-8">''' + policy + '''
<title>Fetch nativo con async/await</title>
<style>body{font:24px sans-serif;padding:48px;background:#edf4f2;color:#18362d}#result{background:white;padding:24px}</style>
<h1>Red nativa y continuaciones Rust</h1><p id="result">Pendiente</p><img src="/hold" alt="">
<script type="application/x-mwr-js">
const result=document.querySelector('#result');
async function run(){
  try {
    const response=await fetch(''' + json.dumps(url) + ''');
    if(response.bodyUsed){throw new Error('body consumed before text');}
    const body=response.text();
    if(!response.bodyUsed){throw new Error('body not marked consumed');}
    const text=await body;
    result.textContent=String(response.status)+'|'+String(response.ok)+'|'+text;
  }catch(error){result.textContent=error.name;}
}
run();
</script>'''
        try:
            with tempfile.TemporaryDirectory(prefix='mwr-fetch-') as temp:
                env = dict(os.environ, MWR_NATIVE_JS='1', MWR_NATIVE_FETCH_TEST='1',
                           MWR_NATIVE_FETCH_EXPECT=expected, MWR_NATIVE_MICROTASK_GC_STRESS='1',
                           LIBGL_ALWAYS_SOFTWARE='1', EGL_PLATFORM='surfaceless')
                for key in ('MWR_NATIVE_SELF_TEST', 'MWR_NATIVE_GC_TEST'):
                    env.pop(key, None)
                for key in ('XDG_RUNTIME_DIR', 'XDG_CACHE_HOME', 'XDG_CONFIG_HOME', 'TMPDIR'):
                    path = Path(temp) / key
                    path.mkdir(mode=0o700)
                    env[key] = str(path)
                image = output / f'{scenario}.png'
                process = subprocess.Popen([str(binary), '--headless', '--window-size=900x600',
                                            '--temporary-storage', '-x', '-o', str(image),
                                            f'http://127.0.0.1:{server.server_port}/'],
                                           env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                lines = []

                def read():
                    for line in process.stdout:
                        lines.append(line)
                        if line.startswith('MWR_NATIVE_FETCH_COMPLETION ') and line.rstrip().endswith(' headers'):
                            headers_seen.set()
                        if line.startswith('MWR_NATIVE_FETCH_PROOF '):
                            release_load.set()

                reader = threading.Thread(target=read, daemon=True)
                reader.start()
                try:
                    code = process.wait(timeout=70)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
                    reader.join(timeout=5)
                    log = ''.join(lines)
                    (output / f'{scenario}.log').write_text(log)
                    (output / f'{scenario}.html').write_text(fixture)
                assert code == 0, (scenario, code, log[-5000:])
                proofs = [json.loads(line.removeprefix('MWR_NATIVE_FETCH_PROOF '))
                          for line in lines if line.startswith('MWR_NATIVE_FETCH_PROOF ')]
                assert proofs and all(p == dict(after=expected, native_fetch=True) for p in proofs), (scenario, log)
                assert 'MWR_NATIVE_ERROR ' not in log, log
                assert image.exists() and image.stat().st_size > 1000
                completions = [line.strip() for line in lines if line.startswith('MWR_NATIVE_FETCH_COMPLETION ')]
                assert len(completions) == (2 if scenario in ('success', 'http404') else 1), (scenario, completions)
                if scenario == 'csp':
                    assert '/data' not in requests, requests
                results[scenario] = dict(after=expected, completions=completions,
                                         requests=requests, joint_gc_before_microtasks=log.count('MWR_NATIVE_MICROTASK_GC\n'))
                print(scenario, 'passed', flush=True)
        finally:
            release_load.set()
            headers_seen.set()
            for service in (server, other):
                service.shutdown()
                service.server_close()
    report = dict(servo_revision='aac43a3f31a259f04a574f5ec4e959c943ad7cc7',
                  binary_sha256=hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
                  module_sha256=hashlib.file_digest((HERE / 'mwr_network.rs').open('rb'), 'sha256').hexdigest(),
                  scenarios=results, native_fetch=True, javascript_transport=False, webdriver=False)
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
