#!/usr/bin/env python3
"""Export the canonical tracked SDK/contracts as a versioned plugins snapshot."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]

def export(output):
    if output.exists():
        raise ValueError('Choose a new SDK export directory')
    changed = subprocess.check_output(['git', '-C', str(ROOT), 'status', '--porcelain', '--', 'crates/guest', 'wit'])
    if changed:
        raise ValueError('Commit SDK/contract changes before exporting')
    revision = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()
    files = subprocess.check_output(['git', '-C', str(ROOT), 'ls-files', '-z', 'crates/guest', 'wit']).decode().split('\0')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.sdk-export-', dir=output.parent) as temp:
        stage = Path(temp) / 'export'
        base = stage / 'vendor/terrarium-sdk'
        base.mkdir(parents=True)
        for name in filter(None, files):
            target = base / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        source = {'host_commit': revision, 'api': 'terrarium:plugin@0.1.0'}
        (base / 'SOURCE.json').write_text(json.dumps(source, indent=2) + '\n')
        hashes = {f.relative_to(base).as_posix(): hashlib.sha256(f.read_bytes()).hexdigest()
                  for f in sorted(base.rglob('*')) if f.is_file()}
        (stage / 'sdk.lock.json').write_text(json.dumps({'source': source, 'files': hashes}, indent=2) + '\n')
        stage.rename(output)
    print(f'Exported SDK from {revision} to {output}')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    export(parser.parse_args().output.resolve())
