#!/usr/bin/env python3
"""Release committed main: bump, validate, build, tag, publish, and verify."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = 'baselsoftwaredev/terminal-diff-viewer'
REQUIRED_ASSETS = {
    'tdv-aarch64-apple-darwin.tar.gz', 'tdv-x86_64-apple-darwin.tar.gz',
    'tdv-aarch64-unknown-linux-musl.tar.gz', 'tdv-x86_64-unknown-linux-musl.tar.gz',
    'tdv-x86_64-pc-windows-msvc.zip', 'SHA256SUMS', 'install.sh',
}


class ReleaseError(Exception):
    pass


def run(*command, capture=False):
    if not capture:
        print('+ ' + ' '.join(str(part) for part in command), flush=True)
    result = subprocess.run(command, cwd=ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else None


def version_tuple(text):
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', text):
        raise ReleaseError('Use a stable version such as 0.3.1.')
    return tuple(int(part) for part in text.split('.'))


def read_version(text):
    package = re.search(r'(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)', text)
    match = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', package[1]) if package else None
    if not match:
        raise ReleaseError('Cannot find the package version in Cargo.toml.')
    version_tuple(match[1])
    return match[1]


def next_version(current, bump, explicit=None):
    numbers = list(version_tuple(current))
    if explicit:
        candidate = explicit[1:] if explicit.startswith('v') else explicit
    else:
        index = {'major': 0, 'minor': 1, 'patch': 2}[bump]
        numbers[index] += 1
        numbers[index + 1:] = [0] * (2 - index)
        candidate = '.'.join(str(part) for part in numbers)
    if version_tuple(candidate) <= version_tuple(current):
        raise ReleaseError('The release version must be newer than ' + current + '.')
    return candidate


def update_manifest(text, version):
    old = read_version(text)
    package = re.search(r'(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)', text)
    body = re.sub(r'(?m)^(version\s*=\s*)"' + re.escape(old) + r'"',
                  lambda match: match[1] + '"' + version + '"', package[1], count=1)
    return text[:package.start(1)] + body + text[package.end(1):]


def preflight(tag, resume):
    for command in ('git', 'cargo', 'gh', 'sh'):
        if shutil.which(command) is None:
            raise ReleaseError('Missing required command: ' + command)
    if Path(run('git', 'rev-parse', '--show-toplevel', capture=True)).resolve() != ROOT:
        raise ReleaseError('Run this script from its own Git checkout.')
    if run('git', 'branch', '--show-current', capture=True) != 'main':
        raise ReleaseError('Release from the main branch, not a detached HEAD or feature branch.')
    if run('git', 'status', '--porcelain', '--untracked-files=normal', capture=True):
        raise ReleaseError('Commit your intended source changes first. The release script only commits version files.')
    origin = run('git', 'remote', 'get-url', 'origin', capture=True)
    allowed = {'git@github.com:' + REPOSITORY + '.git',
               'https://github.com/' + REPOSITORY + '.git',
               'https://github.com/' + REPOSITORY,
               'ssh://git@github.com/' + REPOSITORY + '.git'}
    if origin not in allowed:
        raise ReleaseError('origin must point to ' + REPOSITORY + '.')
    run('gh', 'auth', 'status', '--hostname', 'github.com')
    run('git', 'fetch', 'origin', 'main', '--tags')
    if subprocess.run(['git', 'merge-base', '--is-ancestor', 'origin/main', 'HEAD'], cwd=ROOT).returncode:
        raise ReleaseError('Local main is behind or diverged from origin/main; update it before releasing.')
    existing = subprocess.run(['git', 'rev-parse', '--verify', '--quiet', 'refs/tags/' + tag],
                              cwd=ROOT, stdout=subprocess.DEVNULL).returncode == 0
    if resume:
        if existing and run('git', 'rev-list', '-n', '1', tag, capture=True) != run('git', 'rev-parse', 'HEAD', capture=True):
            raise ReleaseError('--resume requires the existing release tag to point to the current commit.')
        if not existing and run('git', 'log', '-1', '--format=%s', capture=True) != 'Release ' + tag:
            raise ReleaseError('--resume requires an existing release tag or the matching release commit.')
    elif existing:
        raise ReleaseError(tag + ' already exists; tags are never overwritten. Use --resume ' + tag + ' to retry it.')
    return existing


def validate_and_build():
    run('cargo', 'fmt', '--check')
    run('cargo', 'test', '--locked')
    run('cargo', 'clippy', '--locked', '--all-targets', '--', '-D', 'warnings')
    run('sh', '-n', 'install.sh')
    run('sh', '-n', 'scripts/release.sh')
    run(sys.executable, 'scripts/test_install.py')
    run(sys.executable, 'scripts/test_release.py')
    run('cargo', 'build', '--release', '--locked')
    expected = 'tdv ' + read_version((ROOT / 'Cargo.toml').read_text())
    actual = run('cargo', 'run', '--quiet', '--release', '--locked', '--', '--version', capture=True)
    if actual != expected:
        raise ReleaseError('The release binary reported an unexpected version: ' + actual)


def verify_release(tag):
    release = json.loads(run('gh', 'release', 'view', tag, '--repo', REPOSITORY,
                             '--json', 'tagName,isDraft,isPrerelease,assets,url', capture=True))
    names = {asset['name'] for asset in release['assets'] if asset.get('state') == 'uploaded'}
    if release['tagName'] != tag or release['isDraft'] or release['isPrerelease'] or not REQUIRED_ASSETS <= names:
        raise ReleaseError('The release is not fully published with all required downloads.')
    return release['url']


def wait_for_release(tag, resume):
    deadline = time.monotonic() + 120
    while True:
        runs = json.loads(run('gh', 'run', 'list', '--repo', REPOSITORY,
                              '--workflow', 'build.yml', '--branch', tag, '--event', 'push',
                              '--limit', '1', '--json', 'databaseId,status,conclusion', capture=True))
        if runs:
            break
        if time.monotonic() >= deadline:
            raise ReleaseError('GitHub has not started the workflow yet; retry with --resume ' + tag + '.')
        print('Waiting for the GitHub release workflow to start…', flush=True)
        time.sleep(3)
    workflow = runs[0]
    if resume and workflow['status'] == 'completed' and workflow['conclusion'] != 'success':
        run('gh', 'run', 'rerun', str(workflow['databaseId']), '--repo', REPOSITORY)
        time.sleep(3)
    run('gh', 'run', 'watch', str(workflow['databaseId']), '--repo', REPOSITORY,
        '--exit-status', '--compact', '--interval', '15')
    return verify_release(tag)


def release(args):
    manifest = ROOT / 'Cargo.toml'
    original_manifest = manifest.read_text()
    current = read_version(original_manifest)
    if args.resume:
        tag = args.resume if args.resume.startswith('v') else 'v' + args.resume
        if tag != 'v' + current:
            raise ReleaseError('--resume must match the current Cargo.toml version.')
        version = current
    else:
        version = next_version(current, args.bump, args.version)
        tag = 'v' + version
    if args.dry_run:
        print('Dry run: no files, Git refs, or remote services will be changed.')
        print('Repository: ' + REPOSITORY)
        print('Version: ' + current + ' → ' + version)
        print('Validate and build → commit version files → atomic push main + ' + tag)
        print('GitHub builds five platforms → uploads checksums → publishes latest release.')
        return
    tag_exists = preflight(tag, bool(args.resume))
    if not args.resume:
        lock = ROOT / 'Cargo.lock'
        original_lock = lock.read_bytes()
        original_head = run('git', 'rev-parse', 'HEAD', capture=True)
        try:
            manifest.write_text(update_manifest(original_manifest, version))
            # Refresh the root package's lock entry without changing dependency versions online.
            run('cargo', 'check', '--offline')
            validate_and_build()
            run('git', 'add', 'Cargo.toml', 'Cargo.lock')
            run('git', 'commit', '-m', 'Release ' + tag)
        except (Exception, KeyboardInterrupt):
            if run('git', 'rev-parse', 'HEAD', capture=True) == original_head:
                manifest.write_text(original_manifest)
                lock.write_bytes(original_lock)
                # The checkout was clean, so only our own staged version edits are removed.
                subprocess.run(['git', 'restore', '--staged', '--', 'Cargo.toml', 'Cargo.lock'], cwd=ROOT, check=False)
                print('Release preparation failed; restored the original version files.', file=sys.stderr)
            else:
                print('Version commit exists. Retry: sh scripts/release.sh --resume ' + tag, file=sys.stderr)
            raise
    try:
        if not tag_exists:
            run('git', 'tag', '-a', tag, '-m', 'tdv ' + tag)
        # Both refs move together, or neither does. No force-pushes or tag replacement.
        run('git', 'push', '--atomic', 'origin', 'main', 'refs/tags/' + tag)
        if args.no_wait:
            print('Release build started. Finish verification with: sh scripts/release.sh --resume ' + tag)
            return
        url = wait_for_release(tag, bool(args.resume))
    except (Exception, KeyboardInterrupt):
        print('Release unfinished. Retry: sh scripts/release.sh --resume ' + tag, file=sys.stderr)
        raise
    print('\nPublished: ' + url)
    print('Installed tdv versions with startup checks enabled will offer this update on next launch.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--patch', dest='bump', action='store_const', const='patch')
    mode.add_argument('--minor', dest='bump', action='store_const', const='minor')
    mode.add_argument('--major', dest='bump', action='store_const', const='major')
    mode.add_argument('--version', help='Explicit stable version, such as 0.4.0')
    mode.add_argument('--resume', metavar='TAG', help='Retry the current release without bumping its version')
    parser.set_defaults(bump='patch')
    parser.add_argument('--dry-run', action='store_true', help='Show the plan without changing anything or accessing GitHub')
    parser.add_argument('--no-wait', action='store_true', help='Push the release, then return without waiting for GitHub builds')
    args = parser.parse_args()
    try:
        release(args)
    except (ReleaseError, subprocess.CalledProcessError, OSError, ValueError) as error:
        print('Release failed: ' + str(error), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print('Release interrupted.', file=sys.stderr)
        return 130
    return 0


if __name__ == '__main__':
    sys.exit(main())
