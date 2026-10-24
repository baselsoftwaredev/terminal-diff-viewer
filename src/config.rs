use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub syntax: Syntax,
    pub updates: Updates,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Syntax {
    pub languages: Vec<String>,
}

#[derive(Debug, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;
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
