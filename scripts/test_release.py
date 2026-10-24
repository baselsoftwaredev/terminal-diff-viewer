#!/usr/bin/env python3
"""Test release planning and failure handling without any network or publishing."""
import argparse
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import release

MANIFEST = '[package]\nname = "terminal-diff-viewer"\nversion = "0.3.0"\n\n[dependencies]\nexample = "0.3.0"\n'


def options(**overrides):
    values = dict(bump='patch', version=None, resume=None, dry_run=False, no_wait=False)
    values.update(overrides)
    return argparse.Namespace(**values)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='tdv-release-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        (self.root / 'Cargo.toml').write_text(MANIFEST)
        (self.root / 'Cargo.lock').write_bytes(b'original lock')
        self.root_patch = patch.object(release, 'ROOT', self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def test_version_bumps_and_explicit_version(self):
        self.assertEqual(release.next_version('0.3.0', 'patch'), '0.3.1')
        self.assertEqual(release.next_version('0.3.9', 'minor'), '0.4.0')
        self.assertEqual(release.next_version('0.3.9', 'major'), '1.0.0')
        self.assertEqual(release.next_version('0.3.0', 'patch', 'v0.5.0'), '0.5.0')
        for version in ['0.3.0', '0.2.0', '01.4.0', '0.4.0-beta', '../../bad']:
            with self.subTest(version=version), self.assertRaises(release.ReleaseError):
                release.next_version('0.3.0', 'patch', version)

    def test_only_package_version_is_changed(self):
        changed = release.update_manifest(MANIFEST, '0.3.1')
        self.assertEqual(release.read_version(changed), '0.3.1')
        self.assertIn('example = "0.3.0"', changed)

    def test_dry_run_has_no_side_effects_or_network(self):
        with patch.object(release, 'run') as run, contextlib.redirect_stdout(io.StringIO()):
            release.release(options(dry_run=True))
        run.assert_not_called()
        self.assertEqual((self.root / 'Cargo.toml').read_text(), MANIFEST)
        self.assertEqual((self.root / 'Cargo.lock').read_bytes(), b'original lock')

    def test_dirty_checkout_stops_before_authentication(self):
        with patch.object(release.shutil, 'which', return_value='/bin/tool'), \
                patch.object(release, 'run', side_effect=[str(self.root), 'main', ' M src/main.rs']) as run:
            with self.assertRaisesRegex(release.ReleaseError, 'Commit your intended source changes'):
                release.preflight('v0.3.1', False)
        self.assertTrue(all(call.args[0] == 'git' for call in run.call_args_list))

    def test_failed_validation_restores_version_files_and_never_pushes(self):
        with patch.object(release, 'preflight', return_value=False), \
                patch.object(release, 'run') as run, \
                patch.object(release.subprocess, 'run'), \
                patch.object(release, 'validate_and_build', side_effect=release.ReleaseError('test failure')), \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaisesRegex(release.ReleaseError, 'test failure'):
                release.release(options())
        self.assertEqual((self.root / 'Cargo.toml').read_text(), MANIFEST)
        self.assertEqual((self.root / 'Cargo.lock').read_bytes(), b'original lock')
        self.assertFalse(any('push' in call.args for call in run.call_args_list))

    def test_release_stages_only_version_files_and_pushes_atomically(self):
        with patch.object(release, 'preflight', return_value=False), \
                patch.object(release, 'run') as run, \
                patch.object(release, 'validate_and_build'), \
                patch.object(release, 'wait_for_release', return_value='https://example.invalid/release') as wait, \
                contextlib.redirect_stdout(io.StringIO()):
            release.release(options())
        self.assertEqual(release.read_version((self.root / 'Cargo.toml').read_text()), '0.3.1')
        run.assert_any_call('git', 'add', 'Cargo.toml', 'Cargo.lock')
        run.assert_any_call('git', 'push', '--atomic', 'origin', 'main', 'refs/tags/v0.3.1')
        self.assertFalse(any('-A' in call.args or '--force' in call.args for call in run.call_args_list))
        wait.assert_called_once_with('v0.3.1', False)

    def test_resume_never_bumps_or_commits_again(self):
        with patch.object(release, 'preflight', return_value=True), \
                patch.object(release, 'run') as run, \
                patch.object(release, 'validate_and_build') as validate, \
                patch.object(release, 'wait_for_release', return_value='https://example.invalid/release'), \
                contextlib.redirect_stdout(io.StringIO()):
            release.release(options(resume='v0.3.0'))
        validate.assert_not_called()
        self.assertEqual((self.root / 'Cargo.toml').read_text(), MANIFEST)
        self.assertFalse(any('commit' in call.args or 'tag' in call.args for call in run.call_args_list))

    def test_published_release_must_have_all_uploaded_assets(self):
        complete = dict(tagName='v0.3.1', isDraft=False, isPrerelease=False,
                        assets=[dict(name=name, state='uploaded') for name in release.REQUIRED_ASSETS],
                        url='https://example.invalid/release')
        with patch.object(release, 'run', return_value=json.dumps(complete)):
            self.assertEqual(release.verify_release('v0.3.1'), complete['url'])
        for field, value in [('isDraft', True), ('isPrerelease', True), ('assets', [])]:
            incomplete = dict(complete, **{field: value})
            with self.subTest(field=field), patch.object(release, 'run', return_value=json.dumps(incomplete)):
                with self.assertRaises(release.ReleaseError):
                    release.verify_release('v0.3.1')


if __name__ == '__main__':
    unittest.main()
