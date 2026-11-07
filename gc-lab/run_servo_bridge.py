"""Real Servo DOM, external Rust interpreter, explicit SpiderMonkey transport.
No program source/closure body is evaluated by WebDriver; only static DOM RPC.
"""
import argparse
import base64
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request

INIT = '''
window.__mwr = {next: 1, nodes: new Map(), ids: new WeakMap(), hooks: new Map(), queue: []};
return true;
'''
RPC = '''
const s = window.__mwr, op = arguments[0], a = arguments[1];
function node(id) { if (!s.nodes.has(id)) throw Error('released node'); return s.nodes.get(id); }
function register(n) {
  if (!n) throw Error('selector did not match');
  const old = s.ids.get(n);
  if (old && s.nodes.has(old)) return old;
  const id = s.next++; s.ids.set(n, id); s.nodes.set(id, n); return id;
}
switch (op) {
 case 'CREATE': return register(document.createElement(a[0]));
 case 'QUERY': return register(document.querySelector(a[0]));
 case 'TAG': return node(a[0]).tagName;
 case 'TEXT': return node(a[0]).textContent;
 case 'SET_TEXT': node(a[0]).textContent = a[1]; return null;
 case 'LISTEN': {
   const id = a[0], n = node(id), old = s.hooks.get(id);
   if (old) { n.removeEventListener('click', old); s.hooks.delete(id); }
   if (a[1]) { const hook = () => { s.queue.push(id); }; s.hooks.set(id, hook); n.addEventListener('click', hook); }
   return null;
 }
 case 'CLICK': node(a[0]).click(); return s.queue.splice(0);
 case 'RELEASE': {
   for (const id of a) {
     const hook = s.hooks.get(id);
     if (hook && s.nodes.has(id)) node(id).removeEventListener('click', hook);
     s.hooks.delete(id); s.nodes.delete(id);
   }
   s.queue = s.queue.filter(id => s.nodes.has(id)); return null;
 }
 default: throw Error('unsupported DOM command');
}
'''
STATS = "return {nodes: window.__mwr.nodes.size, hooks: window.__mwr.hooks.size, queued: window.__mwr.queue.length};"

class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass

class Driver:
    def __init__(self, port):
        self.base = f'http://127.0.0.1:{port}'
        self.session = None
    def request(self, method, path, data=None):
        req = urllib.request.Request(self.base + path, data=None if data is None else json.dumps(data).encode(),
                                     headers={'Content-Type': 'application/json'}, method=method)
        try:
            with urllib.request.urlopen(req, timeout=20) as response:
                value = json.load(response)['value']
        except urllib.error.HTTPError as error:
            raise RuntimeError(error.read().decode()) from error
        if isinstance(value, dict) and 'error' in value:
            raise RuntimeError(value)
        return value
    def execute(self, source, args=None):
        return self.request('POST', f'/session/{self.session}/execute/sync', {'script': source, 'args': args or []})


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]

def terminate(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--servo', required=True)
    parser.add_argument('--binary', default='gc-lab/target/release/servo_bridge')
    parser.add_argument('--output', default='gc-lab/servo-results')
    options = parser.parse_args()
    root = Path(__file__).resolve().parent
    out = Path(options.output).resolve()
    out.mkdir(parents=True, exist_ok=True)
    runtime = out / 'runtime'
    runtime.mkdir(exist_ok=True)
    runtime.chmod(0o700)
    env = os.environ.copy()
    env.update(XDG_RUNTIME_DIR=str(runtime), XDG_CACHE_HOME=str(out / 'cache'),
               XDG_CONFIG_HOME=str(out / 'config'), TMPDIR=str(runtime),
               LIBGL_ALWAYS_SOFTWARE='1', EGL_PLATFORM='surfaceless')
    server = ThreadingHTTPServer(('127.0.0.1', 0), partial(QuietHandler, directory=str(root / 'servo-fixture')))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    driver = Driver(free_port())
    servo = bridge = None
    operations = []
    report = {'program_executor': 'gc-lab Rust AST interpreter', 'dom': 'Servo 0.7.0',
              'transport': 'SpiderMonkey DOM RPC via WebDriver', 'spidermonkey_replaced': False}
    try:
        with (out / 'servo.log').open('wb') as servo_log, (out / 'bridge.log').open('wb') as bridge_log:
            servo = subprocess.Popen([str(Path(options.servo).resolve()), '--headless',
                                      f'--webdriver={driver.base.rsplit(":", 1)[1]}', '--window-size=900x600',
                                      '--temporary-storage', 'about:blank'], env=env, stdout=servo_log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 30
            while True:
                try:
                    if driver.request('GET', '/status')['ready']:
                        break
                except Exception:
                    if servo.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError('Servo did not start; see servo.log')
                    time.sleep(.1)
            session = driver.request('POST', '/session', {'capabilities': {'alwaysMatch': {'browserName': 'servo'}}})
            driver.session = session['sessionId']
            report['capabilities'] = session['capabilities']
            assert 'Servo/0.7.0' in report['capabilities'].get('userAgent', ''), 'unexpected Servo release'
            driver.request('POST', f'/session/{driver.session}/url', {'url': f'http://127.0.0.1:{server.server_port}/'})
            driver.execute(INIT)
            source = driver.execute("return document.getElementById('mwr-program').textContent;")
            (out / 'program.js').write_text(source)
            report['before'] = driver.execute("return document.getElementById('button').textContent;")
            bridge = subprocess.Popen([str(Path(options.binary).resolve()), str(out / 'program.js')],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=bridge_log, bufsize=0)

            def send(line):
                bridge.stdin.write((line + '\n').encode())
                bridge.stdin.flush()

            def rpc(fields):
                op = fields[1]
                args = fields[2:]
                operations.append(op)
                if op in ['CREATE', 'QUERY']:
                    arguments = [bytes.fromhex(args[0]).decode()]
                elif op in ['TAG', 'TEXT', 'CLICK']:
                    arguments = [int(args[0])]
                elif op == 'SET_TEXT':
                    arguments = [int(args[0]), bytes.fromhex(args[1]).decode()]
                elif op == 'LISTEN':
                    arguments = [int(args[0]), args[1] == '1']
                elif op == 'RELEASE':
                    arguments = [int(x) for x in args[0].split(',') if x]
                else:
                    raise RuntimeError(f'unsupported transport operation: {op}')
                value = driver.execute(RPC, [op, arguments])
                if op in ['CREATE', 'QUERY']:
                    return str(value)
                if op in ['TAG', 'TEXT']:
                    return value.encode().hex()
                if op == 'CLICK':
                    return ','.join(map(str, value))
                return ''

            def drive_until(target):
                deadline = time.monotonic() + 30
                while True:
                    timeout = max(0, deadline - time.monotonic())
                    if not select.select([bridge.stdout], [], [], timeout)[0]:
                        raise TimeoutError(f'Rust bridge waiting for {target}')
                    line = bridge.stdout.readline().decode().rstrip('\r\n')
                    if not line:
                        raise RuntimeError(f'Rust bridge disconnected, exit={bridge.poll()}')
                    fields = line.split('\t')
                    if fields[0] == 'CALL':
                        try:
                            send('OK\t' + rpc(fields))
                        except Exception as error:
                            send('ERR\t' + str(error).encode().hex())
                    elif fields[0] == 'FAIL':
                        raise RuntimeError(bytes.fromhex(fields[1]).decode())
                    elif fields[0] == target:
                        return fields
                    elif fields[0] != 'RELEASED':
                        raise RuntimeError(f'unexpected protocol line: {line}')

            drive_until('READY')
            report['registered'] = driver.execute(STATS)
            assert report['registered'] == {'nodes': 1, 'hooks': 1, 'queued': 0}
            # This is a WebDriver native element click, NOT an execute-script .click().
            element = driver.request('POST', f'/session/{driver.session}/element', {'using': 'css selector', 'value': '#button'})
            element_id = element['element-6066-11e4-a52e-4f735466cecf']
            driver.request('POST', f'/session/{driver.session}/element/{element_id}/click', {})
            report['after_native_click_before_rust'] = driver.execute("return document.getElementById('button').textContent;")
            assert report['after_native_click_before_rust'] == report['before'] == 'Pulsar'
            events = driver.execute('return window.__mwr.queue.splice(0);')
            assert len(events) == 1
            for node in events:
                send(f'EVENT\t{node}')
                drive_until('EVENT_DONE')
            report['after_rust_callback'] = driver.execute("return document.getElementById('button').textContent;")
            assert report['after_rust_callback'] == 'Funciona'
            screenshot = driver.request('GET', f'/session/{driver.session}/screenshot')
            (out / 'button.png').write_bytes(base64.b64decode(screenshot, validate=True))
            send('RELEASE')
            released = drive_until('RELEASED')
            report['rust_after_release'] = dict(zip(['objects', 'metadata', 'roots', 'remote_nodes'], map(int, released[1:5])))
            assert all(v == 0 for v in report['rust_after_release'].values())
            assert len(released) == 6 and not released[5], 'remote release error'
            report['servo_after_release'] = driver.execute(STATS)
            assert report['servo_after_release'] == {'nodes': 0, 'hooks': 0, 'queued': 0}
            driver.request('POST', f'/session/{driver.session}/element/{element_id}/click', {})
            assert driver.execute('return window.__mwr.queue.length;') == 0
            send('QUIT')
            drive_until('BYE')
            assert bridge.wait(timeout=5) == 0
            report['transport_operations'] = operations
            report['success'] = True
            (out / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
            print(json.dumps(report, indent=2, ensure_ascii=False))
    finally:
        terminate(bridge)
        if driver.session:
            try:
                driver.request('DELETE', f'/session/{driver.session}')
            except Exception:
                pass
        terminate(servo)
        server.shutdown()

if __name__ == '__main__':
    main()
