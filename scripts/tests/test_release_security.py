import copy
import importlib.util
import json
from pathlib import Path
import plistlib
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('release_security', ROOT / 'scripts/release_security.py')
security = importlib.util.module_from_spec(spec)
spec.loader.exec_module(security)
A = security.ANDROID


class ReleaseSecurityTests(unittest.TestCase):
    def setUp(self):
        self.resources = ROOT / 'apps/tauri/src-tauri/gen/android/app/src/main/res'
        self.paths = ET.parse(self.resources / 'xml/file_paths.xml').getroot()
        self.extraction = ET.parse(self.resources / 'xml/data_extraction_rules.xml').getroot()
        self.manifest = ET.fromstring('''<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="io.github.rahul445.terrarium"><application android:allowBackup="false" android:fullBackupContent="false" android:usesCleartextTraffic="false" android:dataExtractionRules="@xml/data_extraction_rules"><activity android:name=".MainActivity" android:exported="true"><intent-filter><action android:name="android.intent.action.MAIN"/><category android:name="android.intent.category.LAUNCHER"/></intent-filter></activity><provider android:name="androidx.core.content.FileProvider" android:authorities="io.github.rahul445.terrarium.fileprovider" android:exported="false"><meta-data android:name="android.support.FILE_PROVIDER_PATHS" android:resource="@xml/file_paths"/></provider></application></manifest>''')

    def test_narrow_sharing_and_transfer_exclusion_policy_passes(self):
        self.assertEqual(security.android_findings(self.manifest, self.paths, self.extraction), [])

    def test_broad_provider_and_incomplete_transfer_exclusions_fail(self):
        self.assertTrue(security.android_findings(self.manifest, ET.fromstring('<paths><external-path path="."/></paths>'), self.extraction))
        bad = copy.deepcopy(self.extraction)
        bad.find('device-transfer').remove(bad.find('device-transfer/exclude'))
        self.assertTrue(security.android_findings(self.manifest, self.paths, bad))

    def test_debug_cleartext_exported_components_and_new_permissions_fail(self):
        for name in ['debuggable', 'allowBackup', 'usesCleartextTraffic']:
            manifest = copy.deepcopy(self.manifest)
            manifest.find('application').set(A + name, 'true')
            self.assertTrue(security.android_findings(manifest, self.paths, self.extraction))
        ET.SubElement(self.manifest.find('application'), 'service', {A + 'name': 'UnreviewedService', A + 'exported': 'true'})
        ET.SubElement(self.manifest, 'uses-permission', {A + 'name': 'android.permission.MANAGE_EXTERNAL_STORAGE'})
        self.assertGreaterEqual(len(security.android_findings(self.manifest, self.paths, self.extraction)), 2)

    def test_broken_provider_and_unprotected_dynamic_receiver_fail(self):
        self.manifest.find('application/provider').remove(self.manifest.find('application/provider/meta-data'))
        ET.SubElement(self.manifest, 'uses-permission', {A + 'name': 'io.github.rahul445.terrarium.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'})
        self.assertGreaterEqual(len(security.android_findings(self.manifest, self.paths, self.extraction)), 2)

    def test_mac_signature_metadata_requires_release_identity_runtime_and_timestamp(self):
        display = 'Authority=Developer ID Application: Example\nTeamIdentifier=ABCD123456\nCodeDirectory flags=0x10000(runtime)\nTimestamp=Nov 5, 2026\n'
        self.assertEqual(security.macos_metadata_findings(display, {}), [])
        self.assertTrue(security.macos_metadata_findings('Signature=adhoc\nTeamIdentifier=not set\nCodeDirectory flags=0x0(none)', {}))
        for entitlement in security.UNSAFE_ENTITLEMENTS:
            self.assertTrue(security.macos_metadata_findings(display, {entitlement: True}))

    def test_mac_source_grants_only_microphone_and_has_a_purpose_string(self):
        root = ROOT / 'apps/tauri/src-tauri'
        config = json.loads((root / 'tauri.conf.json').read_text())
        self.assertIs(config['bundle']['macOS']['hardenedRuntime'], True)
        ent = plistlib.loads((root / config['bundle']['macOS']['entitlements']).read_bytes())
        self.assertEqual(ent, {'com.apple.security.device.audio-input': True})
        self.assertTrue(plistlib.loads((root / 'Info.plist').read_bytes())['NSMicrophoneUsageDescription'])
