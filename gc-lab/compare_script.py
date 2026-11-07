"""Compare the explicit JS subset with Node + a declared mock DOM; not real browser conformance."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

HOST = r'''
const vm = require('node:vm');
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => input += chunk);
process.stdin.on('end', () => {
  const {source, binding} = JSON.parse(input);
  class Node {
    constructor(tag) { this.tagName = tag.toUpperCase(); this.textContent = ''; this.listeners = []; }
    addEventListener(event, callback) { if (event !== 'click') throw Error('unsupported'); if (!this.listeners.includes(callback)) this.listeners.push(callback); }
    removeEventListener(event, callback) { if (event !== 'click') throw Error('unsupported'); this.listeners = this.listeners.filter(x => x !== callback); }
    click() { for (const callback of [...this.listeners]) if (this.listeners.includes(callback)) callback.call(this); }
  }
  try {
    const value = vm.runInNewContext('"use strict";\n' + source + '\n;' + binding, {document: {createElement: tag => new Node(tag)}}, {timeout: 1000});
    console.log(JSON.stringify({ok: true, value: typeof value === 'string' ? value : value.textContent}));
  } catch (e) { console.log(JSON.stringify({ok: false})); }
});
'''

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', default='target/release/script_demo')
    parser.add_argument('--node', default='node')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent
    binary = Path(args.binary).resolve()
    cases = json.loads((root / 'examples/script-corpus.json').read_text())
    rows = []
    with tempfile.TemporaryDirectory() as temporary:
        for index, case in enumerate(cases):
            source = Path(temporary) / f'case-{index}.js'
            source.write_text(case['source'])
            actual_run = subprocess.run([str(binary), str(source), case['binding']], text=True, capture_output=True, timeout=5)
            actual = json.loads(actual_run.stdout)
            reference_run = subprocess.run([args.node, '-e', HOST], input=json.dumps(case), text=True, capture_output=True, timeout=5, check=True)
            reference = json.loads(reference_run.stdout)
            match = actual['ok'] == reference['ok'] and (not actual['ok'] or actual['value'] == reference['value'])
            clean = all(actual[key] == 0 for key in ['live_after_release', 'metadata_after_release', 'roots_after_release'])
            assert actual_run.returncode == (0 if actual['ok'] else 1), case['name']
            rows.append({'case': case['name'], 'matches_node_mock': match, 'released_all_objects_and_roots': clean})
    print(json.dumps({'node_version': subprocess.check_output([args.node, '--version'], text=True).strip(), 'cases': rows}, indent=2))
    if not all(row['matches_node_mock'] and row['released_all_objects_and_roots'] for row in rows):
        raise SystemExit(1)

if __name__ == '__main__':
    main()
