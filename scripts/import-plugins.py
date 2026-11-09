#!/usr/bin/env python3
"""Validate a plugin release and atomically stage explicit host build inputs."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[1]
API = 'terrarium:plugin@0.1.0'

def read_inside(root, name):
    path = PurePosixPath(name)
    if path.is_absolute() or not path.parts or '..' in path.parts or '\\' in name or path.as_posix() != name:
        raise ValueError(f'Unsafe bundle path: {name}')
    file = root.joinpath(*path.parts)
    if not file.resolve().is_relative_to(root.resolve()) or not file.is_file():
        raise ValueError(f'Missing or escaping bundle file: {name}')
    return file.read_bytes()

def validate(source):
    raw = read_inside(source, 'bundle.json')
    lock = json.loads(raw)
    if lock.get('format') != 1 or lock.get('api') != API or not lock.get('plugins'):
        raise ValueError('Unsupported or empty plugin bundle')
    result = {}
    for plugin in lock['plugins']:
        name = plugin['id']
        if not re.fullmatch(r'[a-z][a-z0-9-]*', name) or name in result:
            raise ValueError(f'Invalid/duplicate plugin: {name}')
        files = {}
        for relative, expected in plugin['files'].items():
            content = read_inside(source, f'{name}/{relative}')
            if hashlib.sha256(content).hexdigest() != expected:
                raise ValueError(f'Digest mismatch: {name}/{relative}')
            files[relative] = content
        manifest = json.loads(files['manifest.json'])
        required = {'manifest.json', 'dist/plugin.wasm'}
        required.update(v['template'] for kind in ('pages', 'widgets') for v in manifest.get(kind, []))
        if (manifest['id'] != name or manifest['version'] != plugin['version'] or manifest['api'] != API
                or set(files) != required):
            raise ValueError(f'Manifest/file contract mismatch: {name}')
        if not files['dist/plugin.wasm'].startswith(b'\x00asm\x0d\x00\x01\x00'):
            raise ValueError(f'Expected a WebAssembly component: {name}')
        for relative in required - {'manifest.json', 'dist/plugin.wasm'}:
            json.loads(files[relative])
        result[name] = files
    return raw, result

def import_bundle(source, output):
    raw, packages = validate(source)
    if output.exists():
        # Verify existing bytes as well as the lock before accepting a retry.
        existing_raw, existing = validate(output)
        if existing_raw == raw and existing == packages and (output / 'bundled.rs').read_text() == rust_table(packages):
            return
        raise ValueError('A different bundle is already imported; move bundled-plugins aside before importing an update')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.plugin-import-', dir=output.parent) as temp:
        stage = Path(temp) / 'bundle'
        stage.mkdir()
        for name, files in packages.items():
            for relative, content in files.items():
                target = stage / name / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(content)
        (stage / 'bundle.json').write_bytes(raw)
        (stage / 'bundled.rs').write_text(rust_table(packages))
        stage.rename(output)

def rust_table(packages):
    rows = ['const BUNDLED: &[BundledPackage] = &[']
    for name, files in sorted(packages.items()):
        rows.append(f'    ({json.dumps(name)}, &[')
        for relative in sorted(files):
            path = f'/../../../bundled-plugins/{name}/{relative}'
            rows.append(f'        ({json.dumps(relative)}, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), {json.dumps(path)}))),')
        rows.append('    ]),')
    return '\n'.join(rows + ['];', ''])

def import_approved(source, output, lock_path, update_lock=False):
    raw, _ = validate(source)
    if not update_lock and (not lock_path.is_file() or lock_path.read_bytes() != raw):
        raise ValueError('Bundle differs from plugins.lock.json; review it and use --update-lock to approve')
    import_bundle(source, output)
    if update_lock:
        with tempfile.TemporaryDirectory(prefix=".plugin-lock-", dir=lock_path.parent) as temp:
            staged_lock = Path(temp) / "plugins.lock.json"
            staged_lock.write_bytes(raw)
            staged_lock.replace(lock_path)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('release', type=Path)
    parser.add_argument('--update-lock', action='store_true', help='Explicitly approve a new bundle lock after review')
    args = parser.parse_args()
    import_approved(args.release.resolve(), ROOT / 'bundled-plugins', ROOT / 'plugins.lock.json', args.update_lock)
    print('Verified plugin bundle ready for desktop/Android builds')
