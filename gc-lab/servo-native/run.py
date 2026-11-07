#!/usr/bin/env python3
"""Run only the compiled Servo process: no WebDriver, JS RPC or bridge binary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument('--servo', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=HERE.parent / 'native-results')
    args = parser.parse_args()
    binary = args.servo.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    fixture = (HERE.parent / 'servo-fixture/index.html').read_text()
    # The fixture contains exactly one experimental script and no ordinary JS.
    assert fixture.count('<script') == 1
    assert 'application/x-mwr-js' in fixture
    scenarios = [
        ('disabled', False, True, fixture),
        ('waiting', True, False, fixture),
        ('callback', True, True, fixture),
        ('csp-blocked', True, True, fixture.replace('<meta charset="utf-8">',
            '<meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="script-src \'none\'">')),
        ('syntax-error', True, False, fixture.replace('const button =', 'let button =')),
    ]
    results = {}
    for name, enabled, self_test, html in scenarios:
        with tempfile.TemporaryDirectory(prefix='mwr-native-') as temp:
            work = Path(temp)
            env = dict(os.environ)
            env.pop('MWR_NATIVE_JS', None)
            env.pop('MWR_NATIVE_SELF_TEST', None)
            for key, subdir in [('XDG_RUNTIME_DIR', 'runtime'), ('XDG_CACHE_HOME', 'cache'),
                                ('XDG_CONFIG_HOME', 'config'), ('TMPDIR', 'tmp')]:
                path = work / subdir
                path.mkdir(mode=0o700)
                env[key] = str(path)
            env.update(LIBGL_ALWAYS_SOFTWARE='1', EGL_PLATFORM='surfaceless')
            if enabled:
                env['MWR_NATIVE_JS'] = '1'
            if self_test:
                env['MWR_NATIVE_SELF_TEST'] = '1'
            page = work / 'index.html'
            page.write_text(html)
            image = output / f'{name}.png'
            command = [str(binary), '--headless', '--window-size=900x600',
                       '--temporary-storage', '-x', '-o', str(image), page.as_uri()]
            process = subprocess.run(command, env=env, capture_output=True, timeout=180)
            log = process.stdout.decode(errors='replace') + process.stderr.decode(errors='replace')
            (output / f'{name}.log').write_text(log)
            assert process.returncode == 0, (name, process.returncode, log[-5000:])
            assert image.exists() and image.stat().st_size > 1000, (name, 'missing render')
            executed = log.count('MWR_NATIVE_EXECUTED')
            callbacks = log.count('MWR_NATIVE_CALLBACK')
            errors = [line for line in log.splitlines() if line.startswith('MWR_NATIVE_ERROR ')]
            proofs = [json.loads(line.removeprefix('MWR_NATIVE_PROOF '))
                      for line in log.splitlines() if line.startswith('MWR_NATIVE_PROOF ')]
            expected = {'disabled': (0, 0, 0), 'waiting': (1, 0, 0),
                        'callback': (1, 1, 0), 'syntax-error': (0, 0, 1), 'csp-blocked': (0, 0, 0)}[name]
            assert (executed, callbacks, len(errors)) == expected, (name, log)
            assert log.count('MWR_NATIVE_TEST_TASK') == (1 if name == 'callback' else 0)
            assert len(proofs) == (1 if name == 'callback' else 0), (name, log)
            if proofs:
                assert proofs[0] == dict(before='Pulsar', after='Funciona', objects=0,
                                         roots=0, wrappers=0, metadata=0, leases=0,
                                         click='rust-dom-synthetic')
            results[name] = dict(executions=executed, callbacks=callbacks,
                                 errors=errors, proofs=proofs)
    report = dict(
        servo_revision='aac43a3f31a259f04a574f5ec4e959c943ad7cc7',
        binary_sha256=hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
        patch_sha256=hashlib.file_digest((HERE / 'servo.patch').open('rb'), 'sha256').hexdigest(),
        callback_after_event_loop_turn=True,
        module_sha256=hashlib.file_digest((HERE / 'mwr.rs').open('rb'), 'sha256').hexdigest(),
        in_process=True, webdriver=False, javascript_transport=False,
        input='Rust HTMLElement::Click; synthetic, not hardware input',
        scenarios=results,
    )
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
