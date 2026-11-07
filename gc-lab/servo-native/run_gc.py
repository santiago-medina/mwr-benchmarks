#!/usr/bin/env python3
"""Run real Servo/VM cycle collection; no WebDriver or foreign JS execution."""
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
    parser.add_argument('--iterations', type=int, default=1500)
    parser.add_argument('--output', type=Path, default=HERE.parent / 'native-gc-results')
    args = parser.parse_args()
    assert args.iterations >= 1
    binary = args.servo.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='mwr-joint-gc-') as temp:
        work = Path(temp)
        env = dict(os.environ)
        env.pop('MWR_NATIVE_SELF_TEST', None)
        for key, subdir in [('XDG_RUNTIME_DIR', 'runtime'), ('XDG_CACHE_HOME', 'cache'),
                            ('XDG_CONFIG_HOME', 'config'), ('TMPDIR', 'tmp')]:
            path = work / subdir
            path.mkdir(mode=0o700)
            env[key] = str(path)
        env.update(LIBGL_ALWAYS_SOFTWARE='1', EGL_PLATFORM='surfaceless',
                   MWR_NATIVE_JS='1', MWR_NATIVE_GC_TEST='1',
                   MWR_NATIVE_GC_ITERATIONS=str(args.iterations))
        command = [str(binary), '--headless', '--window-size=900x600', '--temporary-storage',
                   '-x', '-o', str(output / 'cycles.png'), (HERE / 'gc-fixture.html').as_uri()]
        try:
            process = subprocess.run(command, env=env, capture_output=True, timeout=600)
        except subprocess.TimeoutExpired as error:
            (output / 'servo.log').write_bytes((error.stdout or b'') + (error.stderr or b''))
            raise
        log = process.stdout.decode(errors='replace') + process.stderr.decode(errors='replace')
        (output / 'servo.log').write_text(log)
        assert process.returncode == 0, (process.returncode, log[-6000:])
        errors = [line for line in log.splitlines() if line.startswith('MWR_NATIVE_ERROR ')]
        assert not errors, errors
        assert (output / 'cycles.png').exists() and (output / 'cycles.png').stat().st_size > 1000
        proofs = [json.loads(line.removeprefix('MWR_GC_PROOF '))
                  for line in log.splitlines() if line.startswith('MWR_GC_PROOF ')]
        assert len(proofs) == 1, log[-6000:]
        proof = proofs[0]
        assert proof['iterations'] == args.iterations
        assert proof['callbacks'] == args.iterations
        assert proof['native_buttons_finalized'] == 2 * args.iterations
        assert proof['recovered_errors'] == (args.iterations + 16) // 17
        assert proof['baseline_vm_objects'] == proof['final_vm_objects']
        assert proof['persistent_native_handles'] == 1
        assert proof['escaped_invocation_pins'] == 0
        assert proof['manual_global_release'] is False
        assert proof['native_to_vm_to_native_survival'] is True
        assert proof['disconnected_js_root_survival'] is True
        assert proof['stale_ids_rejected'] is True
        assert proof['navigation_rounds'] == 2
        assert proof['old_sessions_destroyed'] is True
        assert proof['old_documents_finalized'] is True
        assert proof['old_global_native_nodes_finalized'] is True
        assert log.count('MWR_GC_NAVIGATION ') == 2
        assert log.count('MWR_NATIVE_CALLBACK') == args.iterations
    report = dict(
        servo_revision='aac43a3f31a259f04a574f5ec4e959c943ad7cc7',
        binary_sha256=hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest(),
        patch_sha256=hashlib.file_digest((HERE / 'servo.patch').open('rb'), 'sha256').hexdigest(),
        harness_sha256=hashlib.file_digest((HERE / 'mwr_gc_test.rs').open('rb'), 'sha256').hexdigest(),
        module_sha256=hashlib.file_digest((HERE / 'mwr.rs').open('rb'), 'sha256').hexdigest(),
        vm_sources_sha256=hashlib.sha256(b''.join(
            str(p.relative_to(HERE.parent / 'src')).encode() + b'\0' + p.read_bytes()
            for p in sorted((HERE.parent / 'src').rglob('*.rs')))).hexdigest(),
        native_finalization_observed=True, in_process=True, webdriver=False,
        javascript_transport=False, proof=proof,
    )
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
