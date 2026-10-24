#!/usr/bin/env python3
"""Package a built binary with its license, docs, and config example."""
import argparse
import json
from pathlib import Path
import subprocess
import tarfile
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--tag', help='Release tag, which must match Cargo.toml')
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], text=True))
    package = next(p for p in metadata['packages'] if p['name'] == 'terminal-diff-viewer')
    if args.tag and args.tag != 'v' + package['version']:
        parser.error('tag must match the Cargo.toml version: v' + package['version'])
    executable = 'tdv.exe' if 'windows' in args.target else 'tdv'
    files = [Path('target') / args.target / 'release' / executable,
             Path('README.md'), Path('LICENSE'), Path('.tdv.toml.example')]
    for path in files:
        if not path.is_file():
            parser.error('missing file: ' + str(path))
    Path('dist').mkdir(exist_ok=True)
    if executable.endswith('.exe'):
        output = Path('dist') / ('tdv-' + args.target + '.zip')
        with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            for path in files:
                archive.write(path, arcname=path.name)
    else:
        output = Path('dist') / ('tdv-' + args.target + '.tar.gz')
        with tarfile.open(output, 'w:gz') as archive:
            for path in files:
                archive.add(path, arcname=path.name, recursive=False)
    print(output)


if __name__ == '__main__':
    main()
