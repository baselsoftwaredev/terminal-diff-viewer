mod config;
mod diff;
mod file_tree;
mod highlight;
mod refresh;
mod search;
mod setup;
mod ui;
mod update;

use std::{
    env,
    io::{self, IsTerminal},
    path::PathBuf,
};

#[derive(Default)]
struct Options {
    repo: PathBuf,
    staged: bool,
    base: Option<String>,
    paths: Vec<String>,
    demo: bool,
    all_files: bool,
    plain: bool,
    no_mouse: bool,
    icons: bool,
    icon_override: Option<bool>,
    config: Option<PathBuf>,
    no_syntax: bool,
    syntax_languages: Vec<String>,
    no_update_check: bool,
    check_updates: bool,
    force_setup: bool,
    skip_setup: bool,
    setup: Option<setup::Setup>,
    startup_notice: String,
    no_auto_refresh: bool,
    auto_refresh: bool,
}

const HELP: &str = "tdv — terminal Git diff viewer

Usage: tdv [OPTIONS] [-- PATH...]

  --repo DIRECTORY   Repository to view (default: current directory)
  --all-files        Start with the full repository file explorer
  --staged           Show staged changes instead of unstaged changes
  --base REVISION    Compare against a revision (e.g. HEAD or main)
  --demo             Explore sample changes without a repository
  --plain            Print the patch without an interactive interface
  --no-mouse         Keep the terminal's normal mouse selection behavior
  --icons            Show file-type icons (requires a Nerd Font v3+)
  --no-icons         Hide icons, overriding the config
  --config FILE      Use a config file instead of the repository's .tdv.toml
  --no-syntax        Disable syntax highlighting, overriding the config
  --no-update-check  Skip the background check for newer releases
  --no-auto-refresh  Refresh only when r is pressed
  --setup            Configure this repository's local preferences
  --no-setup         Skip first-run setup without creating local files
  -h, --help         Show this help
  -V, --version      Show version

Keys: arrows / hjkl navigate, Tab focus, [ / ] files, n / p hunks,
      v current/unified/split, Enter previous code, f sidebar, / or Ctrl+P picker,
      e changed/all files, Ctrl+F find text, F3/Shift+F3 next/previous match,
      r refresh, Shift+R reload config,
      s staged/unstaged, ? help, q quit. Double-click a file to expand.
      Double-Shift opens the picker on terminals with enhanced key reporting.

Requires Git for repository diffs. New untracked files are included in unstaged views.
Syntax highlighting is off by default. Enable languages in .tdv.toml:
  [syntax]
  languages = [\"csharp\", \"cpp\", \"rust\"]
";

fn options() -> io::Result<Option<Options>> {
    let mut opts = Options {
        repo: env::current_dir()?,
        ..Options::default()
    };
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("tdv {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--repo" => {
                opts.repo = PathBuf::from(
                    args.next()
                        .ok_or_else(|| io::Error::other("--repo requires a directory"))?,
                )
            }
            "--base" => {
                opts.base = Some(
                    args.next()
                        .ok_or_else(|| io::Error::other("--base requires a revision"))?,
                )
            }
            "--staged" => opts.staged = true,
            "--all-files" => opts.all_files = true,
            "--demo" => opts.demo = true,
            "--plain" => opts.plain = true,
            "--no-mouse" => opts.no_mouse = true,
            "--icons" => opts.icon_override = Some(true),
            "--no-icons" => opts.icon_override = Some(false),
            "--no-syntax" => opts.no_syntax = true,
            "--no-update-check" => opts.no_update_check = true,
            "--no-auto-refresh" => opts.no_auto_refresh = true,
            "--setup" => opts.force_setup = true,
            "--no-setup" => opts.skip_setup = true,
            "--config" => {
                opts.config =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        io::Error::other("--config requires a file path")
                    })?))
            }
            "--" => {
                opts.paths.extend(args);
                break;
            }
            _ => {
                return Err(io::Error::other(format!(
                    "unknown argument: {arg}; see tdv --help"
                )))
            }
        }
    }
    if opts.force_setup && (opts.skip_setup || opts.config.is_some() || opts.demo || opts.plain) {
        return Err(io::Error::other(
            "--setup cannot be combined with --no-setup, --config, --demo, or --plain",
        ));
    }
    Ok(Some(opts))
}

impl Options {
    fn snapshot(&self) -> io::Result<diff::Snapshot> {
        if self.demo {
            Ok(diff::Snapshot::from_patch(diff::DEMO))
        } else {
            diff::load(&self.repo, self.staged, self.base.as_deref(), &self.paths)
        }
    }
}

fn run() -> io::Result<()> {
    let Some(mut opts) = options()? else {
        return Ok(());
    };
    if opts.plain || !io::stdout().is_terminal() {
        if opts.force_setup {
            return Err(io::Error::other("--setup requires an interactive terminal"));
        }
        print!("{}", opts.snapshot()?.patch);
        return Ok(());
    }
    if !io::stdin().is_terminal() || env::var("TERM").is_ok_and(|v| v == "dumb") {
        return Err(io::Error::other(
            "interactive mode needs a terminal; use --plain",
        ));
    }
    let config = config::load(&opts.repo, opts.config.as_deref())?;
    if !opts.demo && opts.config.is_none() && !opts.skip_setup {
        let target = config::LocalConfig::discover(&opts.repo)?;
        let exists = target.path.try_exists()?;
        if opts.force_setup || !exists {
            opts.setup = Some(setup::Setup::new(target, config.clone(), exists));
        } else if let Err(error) = target.ensure_ignored() {
            opts.startup_notice = format!("Local config: {error}");
        }
    }
    if !opts.no_syntax {
        opts.syntax_languages = config.syntax.languages;
    }
    opts.icons = opts.icon_override.unwrap_or(config.ui.icons);
    opts.check_updates = !opts.no_update_check && config.updates.check_on_startup;
    opts.auto_refresh = !opts.no_auto_refresh && config.refresh.auto && !opts.demo;
    let mut snapshot = opts.snapshot()?;
    if !opts.demo {
        diff::add_repository_files(&mut snapshot, &opts.repo, opts.staged, &opts.paths)?;
    }
    ui::run(opts, snapshot)
}

fn main() {
    if let Err(err) = run() {
        eprintln!("tdv: {err}");
        std::process::exit(1);
    }
}
