import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('bundle_import', Path(__file__).resolve().parents[1] / 'import-plugins.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ImportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'release'
        self.output = self.root / 'output'
        self.manifest = {'id': 'test', 'version': '1.0.0', 'api': module.API, 'pages': [{'template': 'ui/main.json'}]}
        files = {'manifest.json': json.dumps(self.manifest).encode(),
                 'ui/main.json': b'{}', 'dist/plugin.wasm': b'\x00asm\x0d\x00\x01\x00'}
        for name, content in files.items():
            target = self.source / 'test' / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        self.lock = {'format': 1, 'api': module.API, 'plugins': [{'id': 'test', 'version': '1.0.0',
                     'files': {name: hashlib.sha256(content).hexdigest() for name, content in files.items()}}]}
        self.write_lock()

    def write_lock(self):
        (self.source / 'bundle.json').write_text(json.dumps(self.lock))

    def test_import_is_idempotent_and_contains_only_declared_files(self):
        (self.source / 'test/secret.env').write_text('never bundle this')
        module.import_bundle(self.source, self.output)
        module.import_bundle(self.source, self.output)
        self.assertFalse((self.output / 'test/secret.env').exists())
        self.assertIn('test/ui/main.json', (self.output / 'bundled.rs').read_text())

    def test_tampered_input_does_not_create_output(self):
        (self.source / 'test/ui/main.json').write_text('{"changed":true}')
        with self.assertRaisesRegex(ValueError, 'Digest mismatch'):
            module.import_bundle(self.source, self.output)
        self.assertFalse(self.output.exists())

    def test_path_traversal_is_rejected(self):
        self.lock['plugins'][0]['files']['../../outside'] = '0' * 64
        self.write_lock()
        with self.assertRaisesRegex(ValueError, 'Unsafe bundle path'):
            module.import_bundle(self.source, self.output)
        self.assertFalse(self.output.exists())

    def test_symlink_escape_is_rejected(self):
        outside = self.root / 'outside.json'
        outside.write_text('{}')
        target = self.source / 'test/ui/main.json'
        target.unlink()
        target.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, 'escaping'):
            module.import_bundle(self.source, self.output)

    def test_missing_required_file_is_rejected(self):
        del self.lock['plugins'][0]['files']['dist/plugin.wasm']
        self.write_lock()
        with self.assertRaisesRegex(ValueError, 'contract mismatch'):
            module.import_bundle(self.source, self.output)

    def test_existing_import_corruption_is_detected(self):
        module.import_bundle(self.source, self.output)
        (self.output / 'test/ui/main.json').write_text('bad')
        with self.assertRaisesRegex(ValueError, 'Digest mismatch'):
            module.import_bundle(self.source, self.output)

    def test_api_mismatch_is_rejected(self):
        self.lock['api'] = 'terrarium:plugin@99.0.0'
        self.write_lock()
        with self.assertRaisesRegex(ValueError, 'Unsupported'):
            module.import_bundle(self.source, self.output)

    def test_unapproved_bundle_preserves_lock_and_has_no_output(self):
        lock = self.root / 'plugins.lock.json'
        lock.write_bytes(b'previous approved bundle')
        with self.assertRaisesRegex(ValueError, 'differs from plugins.lock'):
            module.import_approved(self.source, self.output, lock)
        self.assertEqual(lock.read_bytes(), b'previous approved bundle')
        self.assertFalse(self.output.exists())

    def test_explicit_approval_pins_bundle_and_allows_identical_retry(self):
        lock = self.root / 'plugins.lock.json'
        module.import_approved(self.source, self.output, lock, update_lock=True)
        module.import_approved(self.source, self.output, lock)
        self.assertEqual(lock.read_bytes(), (self.source / 'bundle.json').read_bytes())

if __name__ == '__main__':
    unittest.main()
