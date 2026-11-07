#!/usr/bin/env python3
"""Run the actual Rust async contract program literals in Node's official harness."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from compare_node import NODE
from run import REVISION

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=HERE / 'async-contract-node.json')
    args = parser.parse_args()
    assert (args.source / '.revision').read_text().strip() == REVISION
    contracts = HERE.parent / 'tests/async_await.rs'
    text = contracts.read_text()
    literals = list(re.finditer(r'\bcheck\(("(?:[^"\\]|\\.)*")\);', text))
    assert literals, 'No contract literals found'
    sources = []
    with tempfile.TemporaryDirectory(prefix='mwr-async-oracle-') as temp:
        root = Path(temp)
        (root / 'test').mkdir()
        (root / 'harness').mkdir()
        for name in ('sta.js', 'assert.js'):
            shutil.copyfile(args.source / 'harness' / name, root / 'harness' / name)
        cases = []
        for index, match in enumerate(literals):
            program = json.loads(match.group(1))
            path = f'contract-{index:02d}.js'
            (root / 'test' / path).write_text(program)
            cases.append(dict(path=path, negative=None, asynchronous=False))
            sources.append(dict(path=path, rust_line=text[:match.start()].count('\n') + 1,
                                source_sha256=hashlib.sha256(program.encode()).hexdigest()))
        result = subprocess.run(['node', '-e', NODE],
                                input=json.dumps(dict(source=str(root), cases=cases)),
                                capture_output=True, text=True, check=True, timeout=60)
    report = json.loads(result.stdout)
    report.update(revision=REVISION, contracts_sha256=hashlib.sha256(contracts.read_bytes()).hexdigest(),
                  runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), sources=sources)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ('node_version', 'total', 'passed', 'failures')}, indent=2))
    return bool(report['failures'])


if __name__ == '__main__':
    raise SystemExit(main())
