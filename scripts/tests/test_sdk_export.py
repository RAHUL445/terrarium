import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('export_sdk', Path(__file__).resolve().parents[1] / 'export-sdk.py')
export_sdk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(export_sdk)


class SdkExportTests(unittest.TestCase):
    def test_dirty_sdk_requires_explicit_development_export_and_records_provenance(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'wit').mkdir()
            (root / 'wit/plugin.wit').write_text('synthetic contract')
            with patch.object(export_sdk, 'ROOT', root):
                with patch.object(export_sdk.subprocess, 'check_output', return_value=b' M wit/plugin.wit'):
                    with self.assertRaisesRegex(ValueError, 'Commit SDK/contract'):
                        export_sdk.export(root / 'refused')
                    self.assertFalse((root / 'refused').exists())
                with patch.object(export_sdk.subprocess, 'check_output', side_effect=[b' M wit/plugin.wit', 'abc123\n', b'wit/plugin.wit\0']):
                    export_sdk.export(root / 'development', development=True)
                lock = json.loads((root / 'development/sdk.lock.json').read_text())
                self.assertIs(lock['source']['source_dirty'], True)
                base = root / 'development/vendor/terrarium-sdk'
                self.assertEqual(json.loads((base / 'SOURCE.json').read_text()), lock['source'])
                self.assertEqual(lock['files'], {p.relative_to(base).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in base.rglob('*') if p.is_file()})
                with patch.object(export_sdk.subprocess, 'check_output', side_effect=[b'', 'def456\n', b'wit/plugin.wit\0']):
                    export_sdk.export(root / 'clean')
                self.assertNotIn('source_dirty', json.loads((root / 'clean/sdk.lock.json').read_text())['source'])
