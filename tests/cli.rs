use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct Repo(PathBuf);
static NEXT_REPO: AtomicUsize = AtomicUsize::new(0);
impl Repo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tdv-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_REPO.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let repo = Self(path);
        repo.git(&["init", "--quiet"]);
        repo
    }
    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn tdv(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tdv"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn real_git_staged_unstaged_and_path_filter() {
    let repo = Repo::new();
    fs::write(repo.0.join("hello world.rs"), "old\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "initial",
    ]);
    fs::write(repo.0.join("hello world.rs"), "staged\n").unwrap();
    repo.git(&["add", "."]);
    fs::write(repo.0.join("hello world.rs"), "working\n").unwrap();
    fs::write(repo.0.join("untracked"), "new content\n").unwrap();
    let out = repo.tdv(&[]);
    assert!(out.status.success());
    let patch = String::from_utf8(out.stdout).unwrap();
    assert!(patch.contains("-staged\n+working"));
    assert!(patch.contains("b/untracked"));
    assert!(patch.contains("+new content"));
    assert!(!patch.contains('\x1b'));
    let out = repo.tdv(&["--staged", "--plain"]);
    assert!(out.status.success());
    let staged_patch = String::from_utf8(out.stdout).unwrap();
    assert!(staged_patch.contains("-old\n+staged"));
    assert!(!staged_patch.contains("b/untracked"));
    let out = repo.tdv(&["--base", "HEAD"]);
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("-old\n+working"));
    let out = repo.tdv(&["--", "missing"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(!repo.tdv(&["--base", "--output=oops"]).status.success());
    assert!(!repo.tdv(&["--unknown"]).status.success());
}

#[test]
fn demo_help_and_non_repository_error() {
    let repo = Repo::new();
    assert!(repo.tdv(&["--help"]).status.success());
    let out = repo.tdv(&["--demo", "--plain"]);
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("src/greeting.rs"));
    let out = repo.tdv(&["--repo", "/"]);
    assert!(!out.status.success());
    assert!(!out.stderr.is_empty());
}

#[test]
fn plain_output_and_noninteractive_setup_never_create_preferences() {
    let repo = Repo::new();
    let exclude = repo.0.join(".git/info/exclude");
    let previous = fs::read(&exclude).unwrap();
    assert!(repo.tdv(&["--plain"]).status.success());
    assert!(repo.tdv(&["--no-setup"]).status.success());
    let out = repo.tdv(&["--setup"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("interactive terminal"));
    assert!(!repo.0.join(".tdv.toml").exists());
    assert_eq!(fs::read(&exclude).unwrap(), previous);
    assert!(!repo.tdv(&["--setup", "--no-setup"]).status.success());
}
