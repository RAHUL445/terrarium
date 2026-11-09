import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('dependency_exposure', Path(__file__).parents[1] / 'dependency_exposure.py')
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class ExposureGuardTests(unittest.TestCase):
    def setUp(self):
        self.lock = {'package': [
            {'name': 'im', 'version': '15.1.0', 'checksum': 'reviewed'},
            {'name': 'loro-internal', 'dependencies': ['im 15.1.0']},
        ]}
        self.review = {'packages': [{'name': 'im', 'version': '15.1.0', 'checksum': 'reviewed'}],
                       'exclusive_consumers': {'im': ['loro-internal']}}

    def test_reviewed_lock_passes(self):
        policy.check_lock(self.lock, self.review)

    def test_version_checksum_or_duplicate_requires_review(self):
        for change in ({'version': '16.0.0'}, {'checksum': 'changed'}):
            lock = copy.deepcopy(self.lock)
            lock['package'][0].update(change)
            with self.assertRaises(ValueError):
                policy.check_lock(lock, self.review)
        self.lock['package'].append(dict(self.lock['package'][0]))
        with self.assertRaises(ValueError):
            policy.check_lock(self.lock, self.review)

    def test_local_backport_cannot_silently_revert_to_registry(self):
        lock = {'package': [{'name': 'loro-internal', 'version': '1.17.1'}]}
        review = {'packages': [{'name': 'loro-internal', 'version': '1.17.1',
                                'source_path': 'vendor/loro-internal'}],
                  'exclusive_consumers': {}}
        policy.check_lock(lock, review)
        lock['package'][0]['source'] = 'registry+https://github.com/rust-lang/crates.io-index'
        with self.assertRaises(ValueError):
            policy.check_lock(lock, review)

    def test_new_consumer_requires_review(self):
        self.lock['package'].append({'name': 'another-user', 'dependencies': ['im']})
        with self.assertRaises(ValueError):
            policy.check_lock(self.lock, self.review)

    def test_target_graph_refuses_active_glib_or_changed_path(self):
        graph = 'loro-internal v1.17.1\nimbl v7.0.2\nimbl-sized-chunks v0.2.0 (*)\n'
        policy.check_graph(graph)
        for changed in (graph + 'glib v0.18.5\n', graph + 'im v15.1.0\n', 'loro v2.0.0\n'):
            with self.assertRaises(ValueError):
                policy.check_graph(changed)

    def test_feature_changes_require_review(self):
        graph = 'loro-internal v1.17.1|default\nimbl v7.0.2|serde\nimbl-sized-chunks v0.2.0|default\n'
        expected = {'imbl': 'serde'}
        policy.check_graph(graph, expected)
        with self.assertRaises(ValueError):
            policy.check_graph(graph.replace('|serde', '|pool,serde'), expected)

    def test_all_remaining_advisory_packages_must_stay_inactive(self):
        graph = 'loro-internal v1.17.1\nimbl v7.0.2\nimbl-sized-chunks v0.2.0\n'
        for package in ('glib', 'atomic-polyfill', 'proc-macro-error',
                        'zbus_names', 'zvariant', 'zvariant_derive'):
            with self.subTest(package=package), self.assertRaises(ValueError):
                policy.check_graph(graph + f'{package} v1.0.0\n')

    def test_source_change_and_new_rust_file_invalidate_review(self):
        with tempfile.TemporaryDirectory() as temp:
            registry = Path(temp)
            base = registry / 'test-registry/im-15.1.0'
            (base / 'src').mkdir(parents=True)
            (base / 'Cargo.toml').write_text('[package]\nname="im"\n')
            source = base / 'src/lib.rs'
            source.write_text('// reviewed\n')
            self.review['packages'][0]['reviewed_source_sha256'] = policy.source_digest(base)
            policy.check_sources(registry, self.review)
            source.write_text('// changed\n')
            with self.assertRaises(ValueError):
                policy.check_sources(registry, self.review)
            source.write_text('// reviewed\n')
            (base / 'src/new.rs').write_text('// new path\n')
            with self.assertRaises(ValueError):
                policy.check_sources(registry, self.review)

    def test_missing_source_cannot_pass(self):
        self.review['packages'][0]['reviewed_source_sha256'] = 'missing'
        with tempfile.TemporaryDirectory() as temp, self.assertRaises(ValueError):
            policy.check_sources(Path(temp), self.review)
