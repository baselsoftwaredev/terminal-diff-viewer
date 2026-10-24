use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub syntax: Syntax,
    pub updates: Updates,
    pub refresh: Refresh,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Refresh {
    pub auto: bool,
}
impl Default for Refresh {
    fn default() -> Self {
        Self { auto: true }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Syntax {
    pub languages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Updates {
    pub check_on_startup: bool,
}
impl Default for Updates {
    fn default() -> Self {
        Self {
            check_on_startup: true,
        }
    }
}

pub fn language_name(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "csharp" | "cs" => "c#".into(),
        "cpp" | "cxx" => "c++".into(),
        "rs" => "rust".into(),
        "js" => "javascript".into(),
        "sh" | "bash" | "shell" => "bourne again shell (bash)".into(),
        other => other.to_owned(),
    }
}

fn parse(text: &str) -> io::Result<Config> {
    let mut config: Config =
        toml::from_str(text).map_err(|err| io::Error::other(err.to_string()))?;
    config.syntax.languages = config
        .syntax
        .languages
        .iter()
        .map(|name| language_name(name))
        .collect();
    Ok(config)
}

pub fn default_path(repo: &Path) -> PathBuf {
    let root = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            PathBuf::from(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']))
        });
    root.as_deref().unwrap_or(repo).join(".tdv.toml")
}

pub fn load(repo: &Path, explicit: Option<&Path>) -> io::Result<Config> {
    let path = explicit
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_path(repo));
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if explicit.is_none() && err.kind() == io::ErrorKind::NotFound => {
            return Ok(Config::default())
        }
        Err(err) => return Err(io::Error::other(format!("{}: {err}", path.display()))),
    };
    parse(&text).map_err(|err| io::Error::other(format!("{}: {err}", path.display())))
}

#[derive(Debug)]
pub struct LocalConfig {
    pub path: PathBuf,
    root: PathBuf,
    exclude: PathBuf,
}

fn git_path(repo: &Path, args: &[&str]) -> io::Result<PathBuf> {
    let output = Command::new("git").current_dir(repo).args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim_end_matches(['\r', '\n']),
    ))
}

impl LocalConfig {
    pub fn discover(repo: &Path) -> io::Result<Self> {
        let root = git_path(repo, &["rev-parse", "--show-toplevel"])?;
        // Let Git resolve linked worktrees and .git files, rather than assuming a directory.
        let exclude = git_path(&root, &["rev-parse", "--git-path", "info/exclude"])?;
        let exclude = if exclude.is_absolute() {
            exclude
        } else {
            root.join(exclude)
        };
        Ok(Self {
            path: root.join(".tdv.toml"),
            root,
            exclude,
        })
    }

    pub fn is_tracked(&self) -> io::Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["ls-files", "--error-unmatch", "--", ".tdv.toml"])
            .output()?;
        Ok(output.status.success())
    }

    fn is_ignored(&self) -> io::Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["check-ignore", "--quiet", "--no-index", "--", ".tdv.toml"])
            .output()?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            )),
        }
    }

    pub fn ensure_ignored(&self) -> io::Result<()> {
        if self.is_tracked()? {
            return Err(io::Error::other(".tdv.toml is already tracked; untrack it first or use --config with a file outside the repository"));
        }
        if self.is_ignored()? {
            return Ok(());
        }
        let existing = match fs::read(&self.exclude) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err),
        };
        let rule = b"\n# Local terminal-diff-viewer preferences\n/.tdv.toml\n";
        if !existing.ends_with(rule) {
            if let Some(parent) = self.exclude.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.exclude)?;
            if !existing.is_empty() && !existing.ends_with(b"\n") {
                file.write_all(b"\n")?;
            }
            file.write_all(rule)?;
            file.flush()?;
        }
        if !self.is_ignored()? {
            return Err(io::Error::other("the repository's .gitignore overrides the local exclusion; use --config outside the repository or adjust that rule before saving"));
        }
        Ok(())
    }

    pub fn save(&self, config: &Config, replace: bool) -> io::Result<()> {
        // Establish the ignore rule before making any preferences file visible.
        self.ensure_ignored()?;
        let content = format!(
            "# Local tdv preferences. Managed by tdv --setup.\n{}",
            toml::to_string_pretty(config).map_err(io::Error::other)?
        );
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        temporary.write_all(content.as_bytes())?;
        temporary.flush()?;
        if replace {
            temporary.persist(&self.path).map_err(|e| e.error)?;
        } else {
            temporary
                .persist_noclobber(&self.path)
                .map_err(|e| e.error)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(repo: &Path, args: &[&str]) {
        let result = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fn repository() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet"]);
        root
    }
    #[test]
    fn saved_preferences_are_local_ignored_and_idempotent() {
        let root = repository();
        fs::write(root.path().join(".gitignore"), "/target\n").unwrap();
        let target = LocalConfig::discover(root.path()).unwrap();
        let previous = fs::read(&target.exclude).unwrap();
        let mut config = Config::default();
        config.syntax.languages = vec!["rust".into()];
        config.updates.check_on_startup = false;
        target.save(&config, false).unwrap();
        assert!(target.is_ignored().unwrap());
        assert!(!target.is_tracked().unwrap());
        assert!(fs::read(&target.exclude).unwrap().starts_with(&previous));
        assert_eq!(
            fs::read_to_string(root.path().join(".gitignore")).unwrap(),
            "/target\n"
        );
        let rules = fs::read(&target.exclude).unwrap();
        target.ensure_ignored().unwrap();
        assert_eq!(rules, fs::read(&target.exclude).unwrap());
        let loaded = load(root.path(), None).unwrap();
        assert_eq!(loaded.syntax.languages, ["rust"]);
        assert!(!loaded.updates.check_on_startup);
        let output = Command::new("git")
            .current_dir(root.path())
            .args(["ls-files", "--others", "--exclude-standard"])
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&output.stdout).contains(".tdv.toml"));
    }
    #[test]
    fn first_run_never_overwrites_an_existing_config() {
        let root = repository();
        let target = LocalConfig::discover(root.path()).unwrap();
        fs::write(&target.path, "[syntax]\nlanguages = ['cpp']\n").unwrap();
        assert!(target.save(&Config::default(), false).is_err());
        assert_eq!(load(root.path(), None).unwrap().syntax.languages, ["c++"]);
        target.save(&Config::default(), true).unwrap();
        assert!(load(root.path(), None).unwrap().syntax.languages.is_empty());
    }
    #[test]
    fn tracked_configs_and_overriding_rules_are_not_silently_changed() {
        let root = repository();
        let target = LocalConfig::discover(root.path()).unwrap();
        fs::write(root.path().join(".gitignore"), "!/.tdv.toml\n").unwrap();
        assert!(target.save(&Config::default(), false).is_err());
        assert!(!target.path.exists());
        fs::write(root.path().join(".gitignore"), "").unwrap();
        fs::write(&target.path, "# tracked config\n").unwrap();
        git(root.path(), &["add", "-f", ".tdv.toml"]);
        assert!(target.save(&Config::default(), true).is_err());
        assert_eq!(
            fs::read_to_string(&target.path).unwrap(),
            "# tracked config\n"
        );
    }
    #[test]
    fn linked_worktree_uses_the_correct_exclude_file() {
        let root = repository();
        git(
            root.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "--quiet",
                "-m",
                "initial",
            ],
        );
        let linked = root.path().join("linked");
        git(
            root.path(),
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
        let nested = linked.join("src");
        fs::create_dir(&nested).unwrap();
        let target = LocalConfig::discover(&nested).unwrap();
        target.save(&Config::default(), false).unwrap();
        assert!(target.is_ignored().unwrap());
        assert!(linked.join(".tdv.toml").is_file());
        assert!(!root.path().join(".tdv.toml").exists());
        assert!(fs::read_to_string(root.path().join(".git/info/exclude"))
            .unwrap()
            .contains("/.tdv.toml"));
    }
    #[test]
    fn default_off_and_language_aliases() {
        assert!(parse("").unwrap().syntax.languages.is_empty());
        assert!(parse("[syntax]").unwrap().syntax.languages.is_empty());
        assert!(parse("[syntax]\nlanguages = []")
            .unwrap()
            .syntax
            .languages
            .is_empty());
        assert_eq!(
            parse("[syntax]\nlanguages = ['C#', 'cpp', 'Rust']")
                .unwrap()
                .syntax
                .languages,
            ["c#", "c++", "rust"]
        );
        assert_eq!(language_name("csharp"), "c#");
        assert_eq!(language_name("C++"), "c++");
    }
    #[test]
    fn malformed_config_reports_errors() {
        assert!(parse("[syntax]\nlanguages = true").is_err());
        assert!(parse("[syntax]\nlanguage = ['rust']").is_err());
        assert!(parse("[syntx]").is_err());
    }
    #[test]
    fn startup_checks_are_enabled_unless_explicitly_disabled() {
        assert!(parse("").unwrap().updates.check_on_startup);
        assert!(
            !parse("[updates]\ncheck_on_startup = false")
                .unwrap()
                .updates
                .check_on_startup
        );
        assert!(parse("[updates]\ncheck_on_startup = 'false'").is_err());
    }
    #[test]
    fn automatic_refresh_is_enabled_unless_explicitly_disabled() {
        assert!(parse("").unwrap().refresh.auto);
        assert!(!parse("[refresh]\nauto = false").unwrap().refresh.auto);
    }
    #[test]
    fn root_config_is_found_from_nested_directory_and_explicit_config_wins() {
        let path = std::env::temp_dir().join(format!(
            "tdv-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(path.clone());
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&path)
            .status()
            .unwrap()
            .success());
        let nested = path.join("src/nested");
        fs::create_dir_all(&nested).unwrap();
        assert!(load(&nested, None).unwrap().syntax.languages.is_empty());
        fs::write(path.join(".tdv.toml"), "[syntax]\nlanguages = ['rust']").unwrap();
        assert_eq!(load(&nested, None).unwrap().syntax.languages, ["rust"]);
        let custom = path.join("custom.toml");
        fs::write(&custom, "[syntax]\nlanguages = ['cpp', 'csharp']").unwrap();
        assert_eq!(
            load(&nested, Some(&custom)).unwrap().syntax.languages,
            ["c++", "c#"]
        );
        assert!(load(&nested, Some(&path.join("missing.toml"))).is_err());
        fs::write(&custom, "broken = [").unwrap();
        assert!(load(&nested, Some(&custom))
            .unwrap_err()
            .to_string()
            .contains("custom.toml"));
    }
}
