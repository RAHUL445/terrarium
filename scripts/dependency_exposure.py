#!/usr/bin/env python3
"""Check that a scoped dependency exposure review has not drifted.

This does not suppress advisories or replace cargo-audit. Registry sources must
already be cached; no network access, dependency updates or user data access.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
REVIEW = ROOT / 'docs/security/dependency-exposure.json'
# All remaining findings in the 2026-11-07 full-lock audit. These remain
# findings, not ignores. A supported-target graph must not activate any of them.
INACTIVE_ADVISORY_PACKAGES = frozenset({
    'glib', 'atomic-polyfill', 'proc-macro-error',
    'zbus_names', 'zvariant', 'zvariant_derive',
})


def check_lock(lock, review):
    packages = lock['package']
    for expected in review['packages']:
        found = [p for p in packages if p['name'] == expected['name']]
        if len(found) != 1 or any(found[0].get(k) != expected[k]
                                  for k in ('version', 'checksum') if k in expected):
            raise ValueError(f"Re-review changed dependency: {expected['name']}")
        if 'source_path' in expected and found[0].get('source') is not None:
            raise ValueError(f"Reviewed local backport replaced: {expected['name']}")
    for dependency, expected in review['exclusive_consumers'].items():
        consumers = sorted(p['name'] for p in packages
                           if any(d.split()[0] == dependency
                                  for d in p.get('dependencies', [])))
        if consumers != expected:
            raise ValueError(f'Re-review changed consumers of {dependency}')


def source_digest(base):
    files = sorted(f for f in base.rglob('*.rs') if f.is_file())
    files.append(base / 'Cargo.toml')
    hashes = {f.relative_to(base).as_posix(): hashlib.sha256(f.read_bytes()).hexdigest()
              for f in files}
    encoded = json.dumps(hashes, sort_keys=True, separators=(',', ':')).encode()
    return hashlib.sha256(encoded).hexdigest()


def check_sources(registry, review):
    for package in review['packages']:
        expected = package.get('reviewed_source_sha256')
        if expected is None:
            continue
        name = f"{package['name']}-{package['version']}"
        candidates = ([ROOT / package['source_path']] if 'source_path' in package
                      else list(registry.glob(f'*/{name}')))
        if not candidates or any(source_digest(p) != expected for p in candidates):
            raise ValueError(f'Missing or changed reviewed source: {name}; fetch locked sources and re-review')


def check_graph(output, expected_features=None):
    # Cargo's {p} format is `name vVERSION`, optionally followed by a path or (*).
    packages = {m.group(1) for line in output.splitlines()
                if (m := re.match(r'^(\S+) v[^ ]+', line))}
    if not {'loro-internal', 'imbl', 'imbl-sized-chunks'} <= packages:
        raise ValueError('Reviewed dependency path changed; re-review it')
    if expected_features is not None:
        features = {line.split()[0]: line.split('|', 1)[1].removesuffix(' (*)')
                    for line in output.splitlines()
                    if '|' in line and line.split()[0] in expected_features}
        if features != expected_features:
            raise ValueError('Reviewed dependency features changed; re-review them')
    if {'im', 'sized-chunks'} & packages:
        raise ValueError('Unremediated collections returned to the graph')
    active_findings = sorted(INACTIVE_ADVISORY_PACKAGES & packages)
    if active_findings:
        raise ValueError('Advisory packages became active; review/remediate: '
                         + ', '.join(active_findings))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', action='append', help='Reviewed target; repeat to check several')
    args = parser.parse_args()
    review = json.loads(REVIEW.read_text())
    targets = args.target or review['targets']
    if not set(targets) <= set(review['targets']):
        raise ValueError('Target has no exposure review')
    check_lock(tomllib.loads((ROOT / 'Cargo.lock').read_text()), review)
    cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    check_sources(cargo_home / 'registry/src', review)
    for target in targets:
        process = subprocess.run(
            ['cargo', 'tree', '--locked', '--offline', '--target', target,
             '--edges', 'normal,build', '--prefix', 'none', '--format', '{p}|{f}'],
            cwd=ROOT, capture_output=True, text=True)
        if process.returncode:
            raise ValueError(f'Cannot inspect cached dependency graph for {target}: {process.stderr.strip()}')
        check_graph(process.stdout, review['features_by_target'][target])
        print(f'{target}: reviewed graph/source scope unchanged; '
              'all six remaining advisory packages inactive')
    print('Scope checks passed. This is not an advisory waiver; run the strict cargo-audit gate separately.')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError) as error:
        raise SystemExit(str(error))
