# tdv — terminal diff viewer

A Rust terminal application for browsing Git changes with a mouse or keyboard. The default view shows the full current file, with change markers and a previous-code popup. Includes automatic refresh, new files, optional unified/split diffs, syntax highlighting, line numbers, a file sidebar, a file picker, and change navigation.

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
tdv                         # Working-tree edits and new, untracked files
tdv --staged                # Index compared to HEAD
tdv --base HEAD             # Working-tree changes and new files against HEAD
tdv --base main -- src      # Compare src against main
tdv --repo ../another-repo
tdv --no-mouse              # Preserve terminal text selection
tdv --plain                 # Print a patch without the TUI
tdv --demo                  # Try the interface without a Git repository
tdv --demo --config .tdv.toml.example  # Try Rust syntax colors
tdv --no-syntax             # Override the repository's highlighting config
tdv --no-auto-refresh       # Refresh only when r is pressed
```

Output automatically uses plain text when redirected or piped. A repository with no changes shows an empty state. Interactive views refresh automatically; `r` requests an immediate background refresh.

## Read the full current file

The default **Current** view shows every line of the selected changed file, including unchanged lines outside Git's patch context. The gutter marks `+` for new lines, `~` for replacement lines, and `-` for a deletion boundary beside a surviving line. Click a marked line, or select it with Up/Down and press Enter, to see previous code in a scrollable popup. Added files are labeled `new`; they show their full contents as additions. Ignored files stay hidden.

In unstaged views the current contents come from the working tree, including untracked files. Staged views read the Git index, so unstaged edits do not appear there. Deleted files show a message and let you inspect the previous code. Empty and binary files are listed with a suitable empty state or metadata.

Press `v` to cycle **Current → Unified → Split**. Plain-text output remains a patch and includes new, untracked files for unstaged views. Viewing files never stages them or changes the Git index.

## Automatic refresh

While the interactive viewer is open, a background worker checks Git status and file metadata about once a second. Edits, repeated changes to an already modified file, additions, deletions, staging, and commits trigger a background reload. Unchanged snapshots do not repaint the view. The selected file, horizontal offset, cursor line, and scroll position are retained where possible. A previous-code popup or file picker stays stable until you close it; the newest snapshot is then applied.

Use `--no-auto-refresh` for manual refresh with `r`, or disable it per repository:

```toml
[refresh]
auto = false
```

The initial diff still loads before the UI opens. Later Git commands and file reads run in the background. Large repositories may take longer to refresh; the interface remains usable while a reload is pending. Polling is off in demo, plain-text, and noninteractive modes.

## Enable languages per repository

The first interactive launch in a Git repository opens a local setup screen. Choose C#, C++, Rust, and startup update preferences with the mouse or keyboard. Languages start off; startup update checks start on. Press Enter to save, or Esc to skip for this launch. Update checks wait until you finish or skip setup.

Saving creates `.tdv.toml` in the repository root and adds a local exclusion in Git's `info/exclude`. The config stays on your machine and is excluded from ordinary `git add` and commits. No shared `.gitignore` edit is needed. Git resolves the exclusion path for linked worktrees too. The app also ensures existing, untracked default config is locally ignored on normal interactive launches.

Run `tdv --setup` to reopen setup and save new preferences. Existing files are not automatically overwritten; reconfiguration only writes after you select Save. Additional languages previously configured by hand are preserved. Use `tdv --no-setup` to bypass setup and automatic ignore changes for one launch. Demo mode, redirected output, and `--plain` do not run setup or write local preferences.

If a config is already tracked, tdv does not untrack it automatically. Untrack it yourself or use `--config` with a file outside the repository. If a shared `.gitignore` explicitly forces the file to be included, setup reports the conflict rather than saving preferences that Git would expose.

Syntax highlighting is **off by default for every language**. With no config, the viewer starts no highlighting thread and loads no syntax definitions or themes. Added/removed diff colors always remain available.

The saved file can also be edited directly to opt in to the languages you want:

```toml
[syntax]
languages = ["csharp", "cpp", "rust"]
```

This enables C# (`.cs`, `.csx`), C++ (including `.cpp` and `.hpp`), and Rust (`.rs`). Python, Ruby, Go, and every other language remain unhighlighted. Set `languages = []` to disable highlighting completely. Removing the config causes setup to appear again. [.tdv.toml.example](.tdv.toml.example) shows the supported structure.

Names are case-insensitive. `C#`/`cs` and `C++`/`cxx` are also accepted; other bundled languages can be selected using their Syntect grammar names, such as `Python`, `Ruby`, `Go`, `JavaScript`, `JSON`, or `Markdown`. Names that do not match a bundled grammar enable nothing. You can use `shell` for Bash. Files are detected by extension or recognized filename; there is no content-based language guessing.

Config discovery uses the Git repository root, including when launched from a subfolder or with `--repo`. Outside Git (for example in demo mode), it checks the specified directory. Use `--config /path/to/config.toml` to choose a different file and bypass automatic setup and ignore changes; relative explicit paths resolve from the current working directory. `--no-syntax` overrides the config. Setup choices apply immediately after saving. Restart `tdv` after editing config by hand; `r` refreshes the patch, not the config. Plain-text output ignores highlighting configuration entirely.

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
| Move through code / scroll | Up/Down, j/k, PgUp/PgDn, Space | Wheel over the code |
| Scroll horizontally | Left/Right, h/l | Horizontal wheel or Shift+wheel |
| Switch focus | Tab, Enter to focus diff | Click file list or diff |
| Inspect previous code | Enter on a marked line; Esc closes | Click a changed line; click popup to close |
| Previous/next file | `[` / `]` | Click a file; wheel over file list |
| Previous/next hunk | p / n | |
| Start/end of file | Home/End, g/G | |
| Current/unified/split view | v | Click view in top bar |
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

Unknown languages and large files keep basic colors. Highlighting skips files over 512 KiB or 10,000 lines of combined patch/current text, 8,192 bytes in a line, or 50,000 syntax tokens. The current-file highlighter parses the complete file, preserving multiline context. Diff and previous-code highlighting use available patch context and reset at each hunk, so constructs beginning outside that context may be colored approximately. Cancellation is checked between lines; a slow regex may delay the next file's highlighting, while the interface continues to work.

The file list includes changed tracked files, staged additions, and untracked files in unstaged views. Renames appear as deletion/addition pairs. It does not edit files, stage changes, or browse unchanged files that have no Git changes. File contents are loaded on selection and by background refresh. Paths with unusual characters may appear in Git's quoted format in plain patches. Tabs display as four spaces, and terminal control characters are replaced for display. Plain-text output bypasses syntax highlighting entirely.

Validation:

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
sh -n install.sh
python3 scripts/test_install.py
python3 scripts/test_release.py
python3 scripts/test_terminal.py --binary target/release/tdv  # Optional POSIX terminal smoke test after building
```

Automated tests cover parsing, line alignment, navigation, rendering at several terminal sizes, config discovery and defaults, language selection, update consent and checksum validation, and CLI behavior against a temporary Git repository. CI runs native tests and binary smoke checks on all five release platforms. Interactive terminal behavior is tested locally on macOS; other terminal environments may vary.

## Builds and releases

GitHub Actions runs formatting, lint, installer tests, and native Rust tests, then builds archives for Apple Silicon Mac, Intel Mac, Linux x86_64, Linux ARM64, and Windows x64. Each push to `main`, pull request, and manual workflow run produces downloadable Actions artifacts. Version tags also publish a GitHub Release once every build succeeds.

Commit the intended source changes on `main`, then publish with one command:

```sh
sh scripts/release.sh
```

The script checks your GitHub login and repository state, increments the patch version, updates the lockfile, runs the Rust/installer/release tests and lint checks, builds and smoke-tests the local release binary, commits only the version files, creates the version tag, and pushes `main` and the tag together. It then waits for GitHub to build all five platforms and verifies that the published release has every archive, checksum manifest, and installer. Existing installations with startup checks enabled will offer the update on their next interactive launch. Installing it still requires their confirmation.

Options:

```sh
sh scripts/release.sh --dry-run          # Show the plan without writes or network calls
sh scripts/release.sh --minor            # e.g. 0.3.1 → 0.4.0
sh scripts/release.sh --major            # e.g. 0.4.0 → 1.0.0
sh scripts/release.sh --version 0.5.0     # Choose an explicit newer stable version
sh scripts/release.sh --resume v0.3.1     # Finish or retry an interrupted release
sh scripts/release.sh --no-wait          # Push and return; builds continue on GitHub
```

Run the release command from a clean checkout on `main`, with Rust, Python 3, Git, and an authenticated GitHub CLI available. It refuses to publish uncommitted source changes, overwrite existing tags, or force-push. Validation failures restore the original version files. A failed push or GitHub build can be resumed with the same version instead of creating another version. The release workflow uploads all assets to a draft before publishing; failed builds leave the previous public release available. The workflow uses GitHub's built-in token, so no additional publishing secret is needed.
