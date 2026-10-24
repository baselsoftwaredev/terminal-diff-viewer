#!/usr/bin/env python3
"""Exercise the installer offline against local release fixtures."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[1] / 'install.sh'


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='tdv-installer-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.commands = self.root / 'commands'
        self.commands.mkdir()
        self.fixtures = self.root / 'release'
        self.fixtures.mkdir()
        self.destination = self.root / 'bin with spaces'
        self.env = dict(os.environ)
        self.env.update(PATH=str(self.commands) + os.pathsep + os.environ['PATH'],
                        TDV_BIN_DIR=str(self.destination), TDV_VERSION='latest',
                        TDV_TEST_OS='Darwin', TDV_TEST_ARCH='arm64',
                        TDV_TEST_FIXTURES=str(self.fixtures),
                        TDV_TEST_LOG=str(self.root / 'requests'))
        self.command('uname', '#!/bin/sh\ncase "$1" in\n-s) echo "$TDV_TEST_OS";;\n-m) echo "$TDV_TEST_ARCH";;\nesac\n')
        self.command('curl', '#!' + sys.executable + '''
import os, pathlib, shutil, sys
args = sys.argv[1:]
destination = args[args.index('-o') + 1]
url = args[args.index('-o') - 1]
with open(os.environ['TDV_TEST_LOG'], 'a') as log:
    log.write(url + '\\n')
source = pathlib.Path(os.environ['TDV_TEST_FIXTURES']) / url.rsplit('/', 1)[-1]
if not source.is_file():
    sys.exit(22)
shutil.copyfile(source, destination)
''')

    def command(self, name, contents):
        path = self.commands / name
        path.write_text(contents)
        path.chmod(0o755)

    def fixture(self, target='aarch64-apple-darwin', valid=True, with_checksum=True):
        name = 'tdv-' + target + '.tar.gz'
        path = self.fixtures / name
        with tarfile.open(path, 'w:gz') as archive:
            binary = b'#!/bin/sh\necho "tdv 0.1.0"\n'
            info = tarfile.TarInfo('tdv')
            info.size = len(binary)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(binary))
        digest = hashlib.sha256(path.read_bytes()).hexdigest() if valid else '0' * 64
        (self.fixtures / 'SHA256SUMS').write_text(digest + '  ' + name + '\n' if with_checksum else '')

    def run_installer(self, *args):
        return subprocess.run(['sh', str(INSTALLER), *args], env=self.env,
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              timeout=15)

    def test_platforms_and_paths_with_spaces(self):
        for system, machine, target in [
            ('Darwin', 'arm64', 'aarch64-apple-darwin'),
            ('Darwin', 'x86_64', 'x86_64-apple-darwin'),
            ('Linux', 'x86_64', 'x86_64-unknown-linux-musl'),
            ('Linux', 'aarch64', 'aarch64-unknown-linux-musl'),
        ]:
            with self.subTest(target=target):
                self.env.update(TDV_TEST_OS=system, TDV_TEST_ARCH=machine)
                self.fixture(target)
                out = self.run_installer()
                self.assertEqual(out.returncode, 0, out.stderr)
                installed = self.destination / 'tdv'
                self.assertTrue(os.access(installed, os.X_OK))
                self.assertEqual(subprocess.check_output([str(installed)], text=True).strip(), 'tdv 0.1.0')

    def test_pinned_version_and_directory_override(self):
        self.fixture()
        custom = self.root / 'custom bin'
        out = self.run_installer('--version', 'v0.1.0', '--bin-dir', str(custom))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertTrue((custom / 'tdv').is_file())
        self.assertIn('/releases/download/v0.1.0/', (self.root / 'requests').read_text())

    def test_checksum_failure_preserves_existing_install(self):
        self.destination.mkdir()
        existing = self.destination / 'tdv'
        existing.write_text('existing executable')
        self.fixture(valid=False)
        out = self.run_installer()
        self.assertNotEqual(out.returncode, 0)
        self.assertEqual(existing.read_text(), 'existing executable')
        self.assertEqual(sorted(p.name for p in self.destination.iterdir()), ['tdv'])

    def test_missing_checksum_refuses_install(self):
        self.fixture(with_checksum=False)
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertFalse((self.destination / 'tdv').exists())

    def test_download_failure_refuses_install(self):
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.assertFalse((self.destination / 'tdv').exists())

    def test_bad_arguments_and_unsupported_platform(self):
        for args in [('--version',), ('--version', '../../bad'), ('--bin-dir', ''), ('--unknown',)]:
            with self.subTest(args=args):
                self.assertNotEqual(self.run_installer(*args).returncode, 0)
        self.env['TDV_TEST_OS'] = 'unsupported'
        self.assertNotEqual(self.run_installer().returncode, 0)

    def test_help_does_not_download(self):
        self.assertEqual(self.run_installer('--help').returncode, 0)
        self.assertFalse((self.root / 'requests').exists())


if __name__ == '__main__':
    unittest.main()
