#!/usr/bin/env python3
"""POSIX PTY smoke test for the current-file view and automatic refresh.

Run after building: python3 scripts/test_terminal.py --binary target/release/tdv
"""
import argparse
import codecs
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time


class Screen:
    """Track the cursor/erase sequences emitted by the terminal backend."""
    def __init__(self, width=120, height=26):
        self.width, self.height = width, height
        self.cells = [[' '] * width for _ in range(height)]
        self.row = self.col = 0
        self.pending = ''
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending == '\x1b':
                return
            if self.pending.startswith('\x1b['):
                match = re.match(r'\x1b\[([0-9;?:><]*)([@-~])', self.pending)
                if not match:
                    return
                params, command = match.groups()
                self.pending = self.pending[match.end():]
                numbers = [int(n) if n else 0 for n in params.split(';')] if not any(c in params for c in '?><:') else []
                n = (numbers[0] or 1) if numbers else 1
                if command in ('H', 'f'):
                    self.row = (numbers[0] or 1) - 1 if numbers else 0
                    self.col = (numbers[1] or 1) - 1 if len(numbers) > 1 else 0
                elif command == 'G':
                    self.col = n - 1
                elif command == 'd':
                    self.row = n - 1
                elif command in 'ABCD':
                    if command == 'A': self.row -= n
                    if command == 'B': self.row += n
                    if command == 'C': self.col += n
                    if command == 'D': self.col -= n
                elif command == 'J' and numbers and numbers[0] in (2, 3):
                    self.cells = [[' '] * self.width for _ in range(self.height)]
                elif command == 'K':
                    mode = numbers[0] if numbers else 0
                    lo = 0 if mode in (1, 2) else self.col
                    hi = self.width if mode in (0, 2) else self.col + 1
                    if 0 <= self.row < self.height:
                        self.cells[self.row][lo:hi] = [' '] * (hi - lo)
                continue
            c, self.pending = self.pending[0], self.pending[1:]
            if c == '\x1b':
                if self.pending:
                    self.pending = self.pending[1:]
            elif c == '\r': self.col = 0
            elif c == '\n': self.row += 1
            elif c == '\b': self.col = max(0, self.col - 1)
            elif c >= ' ':
                if 0 <= self.row < self.height and 0 <= self.col < self.width:
                    self.cells[self.row][self.col] = c
                self.col += 1

    def text(self):
        return '\n'.join(''.join(row) for row in self.cells)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/release/tdv')
    args = parser.parse_args()
    if os.name != 'posix':
        parser.error('this optional PTY test requires a POSIX host')
    import fcntl
    import pty
    import select
    import struct
    import termios

    binary = str(Path(args.binary).resolve())
    with tempfile.TemporaryDirectory(prefix='tdv-live-test-') as folder:
        root = Path(folder)
        def git(*args):
            subprocess.run(['git', *args], cwd=root, check=True, stdout=subprocess.DEVNULL)
        git('init', '--quiet')
        config = root / '.tdv.toml'
        config.write_text('[updates]\ncheck_on_startup = false\n')
        with (root / '.git' / 'info' / 'exclude').open('a') as excluded:
            excluded.write('\n/.tdv.toml\n')
        original = ''.join('unchanged line %s\n' % n for n in range(1, 41))
        code = root / 'code.rs'
        code.write_text(original)
        git('add', 'code.rs')
        git('-c', 'user.name=Test', '-c', 'user.email=test@example.com', '-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'initial')
        code.write_text(original.replace('unchanged line 20', 'CURRENT VERSION A'))
        screen = Screen()
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', screen.height, screen.width, 0, 0))
        proc = subprocess.Popen([binary, '--repo', folder, '--no-setup', '--no-update-check'],
                                stdin=slave, stdout=slave, stderr=slave,
                                env=dict(os.environ, TERM='xterm-256color'), start_new_session=True)
        os.close(slave)
        def wait_for(text, timeout=7, present=True):
            deadline = time.monotonic() + timeout
            while (text in screen.text()) != present and time.monotonic() < deadline:
                if select.select([master], [], [], .03)[0]:
                    try: screen.feed(os.read(master, 65536))
                    except OSError: break
            assert (text in screen.text()) == present, 'Unexpected visibility for %r:\n%s' % (text, screen.text())
        def send(keys):
            os.write(master, keys)
        try:
            wait_for('[v] Current')
            wait_for('unchanged line 1')
            send(b'G')
            wait_for('unchanged line 40')
            send(b'g')
            wait_for('unchanged line 1')
            send(b'n')
            wait_for('CURRENT VERSION A')
            send(b'\r')
            wait_for('Previous code')
            wait_for('unchanged line 20')
            code.write_text(original.replace('unchanged line 20', 'CURRENT VERSION B'))
            # Keep the previous-code window pinned while a newer snapshot arrives.
            time.sleep(1.3)
            send(b'\x1b')
            wait_for('CURRENT VERSION B')
            code.write_text(original.replace('unchanged line 20', 'CURRENT VERSION C'))
            wait_for('CURRENT VERSION C')
            added = root / 'extra.rs'
            added.write_text('brand_new_code\nsecond line\n')
            wait_for('extra.rs')
            send(b'\x10extra\r')
            wait_for('brand_new_code')
            send(b'\r')
            wait_for('These lines are new.')
            send(b'\x1b')
            wait_for('Previous code', present=False)
            # Exercise actual SGR mouse input through Crossterm, including drag capture.
            nested = root / 'src'
            nested.mkdir()
            for i in range(36):
                (nested / f'a{i:02}.rs').write_text(f'nested file {i}\n')
            wait_for('▾ src/')
            def mouse(button, col, row, release=False):
                send(f'\x1b[<{button};{col};{row}{"m" if release else "M"}'.encode())
            mouse(0, 3, 3)
            mouse(0, 3, 3, release=True)
            wait_for('▸ src/')
            wait_for('a00.rs', present=False)
            mouse(0, 3, 3)
            mouse(0, 3, 3, release=True)
            wait_for('a00.rs')
            mouse(0, 30, 5)  # Right border of the default 30-column sidebar.
            mouse(32, 45, 5)
            mouse(0, 45, 5, release=True)
            # Resizing leaves room for a longer filename, then drag the scrollbar.
            (nested / 'a00_long_filename_for_resize.rs').write_text('wide sidebar\n')
            wait_for('a00_long_filename_for_resize.rs')
            mouse(0, 44, 3)
            mouse(32, 44, 24)
            mouse(0, 44, 24, release=True)
            wait_for('a35.rs')
            wait_for('brand_new_code')  # Scrolling the tree did not switch files.
            config.write_text('[ui]\nicons = true\n')
            send(b'R')
            wait_for('Reloaded configuration.')
            wait_for('\U000f1617 a35.rs')
            config.write_text('[ui]\nicons = false\n')
            send(b'R')
            wait_for('\U000f1617 a35.rs', present=False)
            send(b's')
            wait_for('No changes in this view.')
            git('add', 'extra.rs')
            wait_for('brand_new_code')
            added.write_text('unstaged_extra_version\n')
            # Staged view must retain the index contents, not leak working-tree edits.
            time.sleep(1.3)
            wait_for('brand_new_code')
            send(b'v')
            wait_for('[v] Unified')
            send(b'v')
            wait_for('[v] Split')
            send(b'q')
            proc.wait(timeout=3)
            assert proc.returncode == 0
            print('PASS: full current file, previous-code popup, repeated live edits, new files, staging, all three views, folder clicks, sidebar resizing, scrollbar dragging, and live configuration reload.')
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=3)
            os.close(master)


if __name__ == '__main__':
    main()
