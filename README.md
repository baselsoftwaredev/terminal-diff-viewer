# tdv — terminal diff viewer

A Rust terminal application for browsing Git changes with a mouse or keyboard. Includes side-by-side and unified views, syntax highlighting, line numbers, a file sidebar, a file picker, and hunk navigation. Repository operations only read diffs.

## Install on another Mac or Linux machine

Download the installer and run it in your terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/baselsoftwaredev/terminal-diff-viewer/main/install.sh -o /tmp/tdv-install.sh
sh /tmp/tdv-install.sh
```

The installer downloads the latest published release for Apple Silicon/Intel Macs or x86_64/ARM64 Linux, verifies its SHA-256 checksum, and installs `tdv` to `~/.local/bin`. Rust and sudo are not required. Installation requires `curl`, `tar`, and `sha256sum` or macOS's built-in `shasum`. Viewing repository diffs requires Git.

If `~/.local/bin` is not already on your PATH, add this line to `~/.zshrc` (or your shell's equivalent), then open a new terminal:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Then run `tdv --demo` to try it, or `tdv` inside a Git repository. The same command works in Terminal.app, iTerm2, VS Code's terminal, and other compatible terminal emulators. Repeat the installer command to update to the latest release.

For a specific version or installation directory:

```sh
sh /tmp/tdv-install.sh --version v0.1.0
sh /tmp/tdv-install.sh --bin-dir "$HOME/bin"
```

You can also download archives directly from [GitHub Releases](https://github.com/baselsoftwaredev/terminal-diff-viewer/releases). Mac binaries target macOS 11 or newer. Linux builds use musl with static linking. For Windows x64, extract the `.zip` and place `tdv.exe` in a directory on your PATH. These binaries are not Apple Developer ID signed or notarized.

The installer requires a published release; Actions artifacts from ordinary commits are available separately in the [Actions tab](https://github.com/baselsoftwaredev/terminal-diff-viewer/actions).

## Build and run

Requires Rust 1.88+ to build and Git on your PATH to view repositories.

```sh
cargo build --release --locked
./target/release/tdv --demo
./target/release/tdv --repo /path/to/your/repository
```

To install the command locally:

```sh
cargo install --path . --locked
tdv
```

The compiled executable does not require Rust to run. Build for each operating system and CPU you want to support.

```sh
tdv                         # Unstaged changes in tracked files
tdv --staged                # Index compared to HEAD
tdv --base HEAD             # Tracked changes compared to HEAD
tdv --base main -- src      # Compare src against main
tdv --repo ../another-repo
tdv --no-mouse              # Preserve terminal text selection
tdv --plain                 # Print a patch without the TUI
tdv --demo                  # Try the interface without a Git repository
tdv --demo --config .tdv.toml.example  # Try Rust syntax colors
tdv --no-syntax             # Override the repository's highlighting config
```

Output automatically uses plain text when redirected or piped. A repository with no changes shows an empty state. Use `r` to refresh after editing files externally.

## Enable languages per repository

Syntax highlighting is **off by default for every language**. With no config, the viewer starts no highlighting thread and loads no syntax definitions or themes. Added/removed diff colors always remain available.

Create `.tdv.toml` at the root of a repository to opt in to the languages you want:

```toml
[syntax]
languages = ["csharp", "cpp", "rust"]
```

This enables C# (`.cs`, `.csx`), C++ (including `.cpp` and `.hpp`), and Rust (`.rs`). Python, Ruby, Go, and every other language remain unhighlighted. Set `languages = []` or remove the file to disable highlighting completely. Copy [.tdv.toml.example](.tdv.toml.example) for a starting point.

Names are case-insensitive. `C#`/`cs` and `C++`/`cxx` are also accepted; other bundled languages can be selected using their Syntect grammar names, such as `Python`, `Ruby`, `Go`, `JavaScript`, `JSON`, or `Markdown`. Names that do not match a bundled grammar enable nothing. You can use `shell` for Bash. Files are detected by extension or recognized filename; there is no content-based language guessing.

Config discovery uses the Git repository root, including when launched from a subfolder or with `--repo`. Outside Git (for example in demo mode), it checks the specified directory. Use `--config /path/to/config.toml` to choose a different file; relative explicit paths resolve from the current working directory. `--no-syntax` overrides the config. Restart `tdv` after changing config; `r` refreshes the patch, not the config. Plain-text output ignores highlighting configuration entirely.

Grammars remain bundled in the executable. When you opt in, the background worker loads the bundled syntax catalog and only parses files whose language is enabled. Config controls runtime highlighting rather than reducing the executable size or removing catalog entries.

## Startup updates

On each interactive launch, `tdv` checks GitHub for a newer stable release in the background, after painting the diff. If one is available for your platform, a prompt asks whether to install it. Press `y` to download, verify the release checksum, and replace the current executable. Press `n`, Esc, or Enter to decline; updates are never installed automatically. You can keep browsing during installation. Once it finishes, restart `tdv` to use the new version.

Offline connections, timeouts, rate limits, or unavailable releases silently skip the check. Download or install failures show a status message. The update requires write access to the installed executable's directory and never asks for sudo. Plain-text output, `--help`, and `--version` do not check for updates. Checks use GitHub's public API with no login or token.

To disable checks for one launch, use `tdv --no-update-check`. To disable them for a repository, add:

```toml
[updates]
check_on_startup = false
```

Checks have a five-second network timeout and never block scrolling or quitting. The update prompt waits until a file picker or help panel is closed.

## Controls

| Action | Keyboard | Mouse |
| --- | --- | --- |
| Scroll vertically | Up/Down, j/k, PgUp/PgDn, Space | Wheel over the diff |
| Scroll horizontally | Left/Right, h/l | Horizontal wheel or Shift+wheel |
| Switch focus | Tab, Enter to focus diff | Click file list or diff |
| Previous/next file | `[` / `]` | Click a file; wheel over file list |
| Previous/next hunk | p / n | |
| Start/end of file | Home/End, g/G | |
| Split/unified view | v | Click view in top bar |
| Staged/unstaged | s | Click source in top bar |
| Find a file | Ctrl+P, `/`, double-Shift where supported | Click Find file in top bar |
| Select in file picker | Up/Down, Enter; Esc closes | |
| Hide/show file list | f | Double-click a file to hide the list |
| Refresh | r | |
| Help | ? | |
| Quit | q, Esc, Ctrl+C | |

The file picker filters by a case-insensitive substring of the path. Double-click means two clicks on the same file within 400 ms. Double-Shift means two standalone Shift presses within 400 ms; it requires a terminal that reports modifier keys through the Kitty keyboard protocol. Ctrl+P and `/` are the portable alternatives.

## Portability and current scope

Uses Ratatui with Crossterm, which supports macOS, Linux, and Windows terminals. Interactive mode requires an actual terminal with cursor positioning and an alternate screen; mouse input and enhanced keyboard reporting depend on the terminal and multiplexer settings. `--plain` works with basic text output, including `TERM=dumb`. The sidebar hides automatically below 65 columns. Below 30 columns or 6 rows, the app asks you to resize.

When enabled, syntax highlighting runs on a background thread using Syntect's bundled grammars and the base16-ocean.dark theme. The diff paints immediately with added/removed colors; syntax colors arrive afterward without needing another keypress. Navigation and quitting never wait for syntax parsing. Old/new sides use separate parser states, diff backgrounds remain visible, and up to four files' results are cached. Refreshing invalidates that cache and rejects stale results. No grammar downloads or external highlighter executable are needed.

Unknown languages and large patches keep plain diff colors. Highlighting skips files over 512 KiB of patch text, 10,000 patch lines, 8,192 bytes in a line, or 50,000 syntax tokens. Highlighting uses available patch context and resets at each hunk, so constructs beginning outside that context may be colored approximately. Cancellation is checked between lines; a slow regex may delay the next file's highlighting, while the interface continues to work.

This initial version shows tracked Git changes, including staged new files. Untracked files are excluded. Renames appear as deletion/addition pairs, binary changes show Git's metadata, and only patch context is displayed. It does not edit files, stage changes, or show an entire unchanged file. Git patch loading and parsing still happen before the initial display and synchronously on refresh; very large diffs may pause during that step. Paths with unusual characters may appear in Git's quoted format. Tabs display as four spaces, and terminal control characters are replaced for display. Plain-text output bypasses syntax highlighting entirely.

Validation:

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
sh -n install.sh
python3 scripts/test_install.py
```

Automated tests cover parsing, line alignment, navigation, rendering at several terminal sizes, config discovery and defaults, language selection, update consent and checksum validation, and CLI behavior against a temporary Git repository. CI runs native tests and binary smoke checks on all five release platforms. Interactive terminal behavior is tested locally on macOS; other terminal environments may vary.

## Builds and releases

GitHub Actions runs formatting, lint, installer tests, and native Rust tests, then builds archives for Apple Silicon Mac, Intel Mac, Linux x86_64, Linux ARM64, and Windows x64. Each push to `main`, pull request, and manual workflow run produces downloadable Actions artifacts. Version tags also publish a GitHub Release once every build succeeds.

To publish a version, first update `Cargo.toml` and run `cargo check` to update `Cargo.lock`, commit those changes, then create and push the matching tag:

```sh
git tag v0.2.0
git push origin main v0.2.0
```

The tag must match the package version. The pipeline uploads all five archives, `SHA256SUMS`, and `install.sh` to a draft release before publishing it. A failed build leaves no new public release. A rerun can complete a draft, but refuses to overwrite an already published release. The workflow uses GitHub's built-in token; no publishing secret is needed.
