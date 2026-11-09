#!/usr/bin/env python3
"""Prove synthetic registry-Loro/backport wire compatibility without app data.

Uses locked, cached dependencies; --fetch-legacy explicitly fetches the locked
legacy test dependencies for CI. It never updates package versions. Builds
an isolated, unpatched registry-Loro reader/writer, then the host's patched one.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / 'crates/core/tests/fixtures/loro-1.17.1'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fetch-legacy', action='store_true',
                        help='Fetch only the locked standalone legacy test dependencies')
    args = parser.parse_args()
    env = dict(os.environ)
    env.setdefault('CARGO_TARGET_DIR', str(ROOT / 'target'))
    with tempfile.TemporaryDirectory(prefix='terrarium-crdt-compat-') as temp:
        root = Path(temp)
        legacy = root / 'legacy'
        (legacy / 'src').mkdir(parents=True)
        shutil.copy(ROOT / 'crates/core/examples/crdt_compat.rs', legacy / 'src/main.rs')
        shutil.copy(FIXTURES / 'verifier.Cargo.lock', legacy / 'Cargo.lock')
        (legacy / 'Cargo.toml').write_text(
            '[package]\nname="legacy-crdt-verifier"\nversion="0.1.0"\nedition="2021"\n'
            '[workspace]\n[dependencies]\nloro="=1.17.1"\n')
        data = root / 'fixtures'
        shutil.copytree(FIXTURES, data)
        def run(args):
            subprocess.run(['cargo', 'run', '--locked', '--offline', '-q', *args],
                           cwd=ROOT, env=env, check=True)
        if args.fetch_legacy:
            subprocess.run(['cargo', 'fetch', '--locked', '--manifest-path', str(legacy / 'Cargo.toml')],
                           cwd=ROOT, env=env, check=True)
        run(['-p', 'terrarium-core', '--example', 'crdt_compat', '--', 'forward', str(data)])
        run(['--manifest-path', str(legacy / 'Cargo.toml'), '--', 'backward', str(data)])
        print('PASS: legacy snapshot/update -> patched merge -> unpatched snapshot/update import')
        print('Synthetic Unicode concurrency, idempotence/reopen and rollback-reader check; no app data used.')


if __name__ == '__main__':
    main()
