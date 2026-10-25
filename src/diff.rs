use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Context,
    Added,
    Removed,
    Header,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub index: usize,
    pub kind: Kind,
    pub text: String,
    pub old: Option<usize>,
    pub new: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct FileDiff {
    pub name: String,
    pub lines: Vec<Line>,
    pub current: Vec<Line>,
    pub changes: Vec<Change>,
    pub note: Option<String>,
    pub is_new: bool,
    pub is_deleted: bool,
    pub unchanged: bool,
    loaded: bool,
    source: Source,
}

#[derive(Clone, Debug, Default)]
enum Source {
    #[default]
    Patch,
    Worktree(PathBuf),
    Index {
        root: PathBuf,
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub start: usize,
    pub end: usize,
    pub kind: ChangeKind,
    pub previous: Vec<Line>,
}

pub struct Snapshot {
    pub patch: String,
    pub files: Vec<FileDiff>,
}
impl Snapshot {
    pub fn from_patch(patch: &str) -> Self {
        Self {
            patch: patch.into(),
            files: parse(patch),
        }
    }
}

impl FileDiff {
    pub fn load_current(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if self.is_deleted {
            self.note = Some(
                "File deleted. Click here or press Enter to inspect the previous code.".into(),
            );
        } else if self
            .lines
            .iter()
            .any(|l| l.text.starts_with("Binary files ") || l.text == "GIT binary patch")
        {
            self.note = Some("Binary file — no text preview.".into());
        } else {
            let bytes = match &self.source {
                Source::Patch => Ok(self
                    .lines
                    .iter()
                    .filter(|l| l.new.is_some())
                    .map(|l| format!("{}\n", l.text))
                    .collect::<String>()
                    .into_bytes()),
                Source::Worktree(path) => read_content(path),
                Source::Index { root, path } => {
                    let mut object = OsString::from(":");
                    object.push(path);
                    Command::new("git")
                        .current_dir(root)
                        .args(["show", "--no-ext-diff", "--no-textconv"])
                        .arg(object)
                        .output()
                        .and_then(|out| {
                            if out.status.success() {
                                Ok(out.stdout)
                            } else {
                                Err(io::Error::other(
                                    String::from_utf8_lossy(&out.stderr).into_owned(),
                                ))
                            }
                        })
                }
            };
            match bytes {
                Ok(bytes) if !bytes.contains(&0) => {
                    self.current = String::from_utf8_lossy(&bytes)
                        .lines()
                        .enumerate()
                        .map(|(index, text)| Line {
                            index,
                            kind: Kind::Context,
                            text: safe_text(text),
                            old: None,
                            new: Some(index + 1),
                        })
                        .collect();
                    if self.current.is_empty() {
                        self.note = Some("Empty file.".into());
                    }
                }
                Ok(_) => self.note = Some("Binary file — no text preview.".into()),
                Err(error) => self.note = Some(format!("Cannot load current file: {error}")),
            }
        }
        self.build_changes();
    }

    fn build_changes(&mut self) {
        self.changes.clear();
        let mut i = 0;
        while i < self.lines.len() {
            if !matches!(self.lines[i].kind, Kind::Added | Kind::Removed) {
                i += 1;
                continue;
            }
            let begin = i;
            while i < self.lines.len() && matches!(self.lines[i].kind, Kind::Added | Kind::Removed)
            {
                i += 1;
            }
            let block = &self.lines[begin..i];
            let added: Vec<_> = block.iter().filter(|l| l.kind == Kind::Added).collect();
            let removed = block.iter().any(|l| l.kind == Kind::Removed);
            let start = added
                .first()
                .and_then(|l| l.new)
                .or_else(|| self.lines[i..].iter().find_map(|l| l.new))
                .unwrap_or(self.current.len().max(1))
                .saturating_sub(1)
                .min(self.current.len().saturating_sub(1));
            let end = (start + added.len().max(1)).min(self.current.len().max(1));
            let kind = if added.is_empty() {
                ChangeKind::Removed
            } else if removed {
                ChangeKind::Modified
            } else {
                ChangeKind::Added
            };
            let previous = self.lines[begin.saturating_sub(3)..(i + 3).min(self.lines.len())]
                .iter()
                .filter(|l| l.old.is_some())
                .cloned()
                .collect();
            self.changes.push(Change {
                start,
                end,
                kind,
                previous,
            });
        }
        if self.is_new && self.changes.is_empty() {
            self.changes.push(Change {
                start: 0,
                end: 1,
                kind: ChangeKind::Added,
                previous: Vec::new(),
            });
        }
    }

    pub fn change_at(&self, line: usize) -> Option<&Change> {
        self.changes
            .iter()
            .find(|change| change.start <= line && line < change.end)
    }
    #[cfg(test)]
    pub fn counts(&self) -> (usize, usize) {
        (
            self.lines.iter().filter(|l| l.kind == Kind::Added).count(),
            self.lines
                .iter()
                .filter(|l| l.kind == Kind::Removed)
                .count(),
        )
    }

    pub fn rows(&self, split: bool) -> Vec<(Option<&Line>, Option<&Line>)> {
        if !split {
            return self.lines.iter().map(|l| (Some(l), None)).collect();
        }
        let mut rows = Vec::new();
        let mut i = 0;
        while i < self.lines.len() {
            let line = &self.lines[i];
            if matches!(line.kind, Kind::Added | Kind::Removed) {
                let mut old = Vec::new();
                let mut new = Vec::new();
                while i < self.lines.len()
                    && matches!(self.lines[i].kind, Kind::Added | Kind::Removed)
                {
                    if self.lines[i].kind == Kind::Removed {
                        old.push(&self.lines[i]);
                    } else {
                        new.push(&self.lines[i]);
                    }
                    i += 1;
                }
                for n in 0..old.len().max(new.len()) {
                    rows.push((old.get(n).copied(), new.get(n).copied()));
                }
            } else {
                rows.push((Some(line), Some(line)));
                i += 1;
            }
        }
        rows
    }
}

fn start(range: &str) -> Option<usize> {
    range.get(1..)?.split(',').next()?.parse().ok()
}

pub fn parse(patch: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    for text in patch.lines() {
        if let Some(name) = text.strip_prefix("diff --git ") {
            files.push(FileDiff {
                name: name
                    .rsplit_once(" b/")
                    .map(|(_, path)| path)
                    .unwrap_or(name)
                    .to_owned(),
                lines: Vec::new(),
                ..FileDiff::default()
            });
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if !in_hunk && (text.starts_with("--- ") || text.starts_with("+++ ")) {
            let path = &text[4..];
            if text == "--- /dev/null" {
                file.is_new = true;
            }
            if text == "+++ /dev/null" {
                file.is_deleted = true;
            }
            if path != "/dev/null" {
                file.name = path
                    .strip_prefix("a/")
                    .or_else(|| path.strip_prefix("b/"))
                    .unwrap_or(path)
                    .to_owned();
            }
            continue;
        }
        let (kind, old_no, new_no, content) = if text.starts_with("@@ ") {
            let mut ranges = text.split_whitespace().skip(1);
            if let (Some(a), Some(b)) =
                (ranges.next().and_then(start), ranges.next().and_then(start))
            {
                old = a;
                new = b;
                in_hunk = true;
            }
            (Kind::Header, None, None, text)
        } else if in_hunk && text.starts_with('-') {
            let n = old;
            old += 1;
            (Kind::Removed, Some(n), None, &text[1..])
        } else if in_hunk && text.starts_with('+') {
            let n = new;
            new += 1;
            (Kind::Added, None, Some(n), &text[1..])
        } else if in_hunk && text.starts_with(' ') {
            let (a, b) = (old, new);
            old += 1;
            new += 1;
            (Kind::Context, Some(a), Some(b), &text[1..])
        } else {
            (Kind::Header, None, None, text)
        };
        // Never forward terminal escape sequences from repository content.
        if text.starts_with("new file mode ") {
            file.is_new = true;
        }
        if text.starts_with("deleted file mode ") {
            file.is_deleted = true;
        }
        let safe = safe_text(content);
        file.lines.push(Line {
            index: file.lines.len(),
            kind,
            text: safe,
            old: old_no,
            new: new_no,
        });
    }
    files
}

fn diff_command(
    repo: &Path,
    staged: bool,
    base: Option<&str>,
    paths: &[String],
    names: bool,
) -> io::Result<Command> {
    let mut cmd = Command::new("git");
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.current_dir(repo).args([
        "--no-pager",
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--no-relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]);
    if names {
        cmd.args(["--name-only", "-z"]);
    } else {
        cmd.args(["--patch", "--unified=3"]);
    }
    if staged {
        cmd.arg("--cached");
    }
    if let Some(base) = base {
        if base.starts_with('-') || base.contains("..") {
            return Err(io::Error::other(
                "--base requires a single revision, without options or revision ranges",
            ));
        }
        cmd.arg(base);
    }
    cmd.arg("--").args(paths);
    Ok(cmd)
}

pub fn load(
    repo: &Path,
    staged: bool,
    base: Option<&str>,
    paths: &[String],
) -> io::Result<Snapshot> {
    let output = diff_command(repo, staged, base, paths, false)?.output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let mut patch = String::from_utf8_lossy(&output.stdout).into_owned();
    let root = repository_root(repo)?;
    let names = diff_command(repo, staged, base, paths, true)?.output()?;
    if !names.status.success() {
        return Err(io::Error::other("cannot list changed files"));
    }
    let mut files = parse(&patch);
    let names = git_paths(&names.stdout);
    if names.len() == files.len() {
        for (file, path) in files.iter_mut().zip(names) {
            file.name = path.to_string_lossy().into_owned();
            file.source = if staged {
                Source::Index {
                    root: root.clone(),
                    path,
                }
            } else {
                Source::Worktree(root.join(path))
            };
        }
    } else {
        for file in &mut files {
            file.source = if staged {
                Source::Index {
                    root: root.clone(),
                    path: PathBuf::from(&file.name),
                }
            } else {
                Source::Worktree(root.join(&file.name))
            };
        }
    }
    if !staged {
        let untracked = Command::new("git")
            .current_dir(if paths.is_empty() { &root } else { repo })
            .args([
                "ls-files",
                "--others",
                "--exclude-standard",
                "--full-name",
                "-z",
                "--",
            ])
            .args(paths)
            .output()?;
        if !untracked.status.success() {
            return Err(io::Error::other("cannot list new files"));
        }
        for path in git_paths(&untracked.stdout) {
            let full_path = root.join(&path);
            let metadata = match fs::symlink_metadata(&full_path) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if !metadata.is_file() && !metadata.file_type().is_symlink() {
                continue;
            }
            let bytes = read_content(&full_path)?;
            let new_patch = new_file_patch(&path, &bytes, &metadata);
            let mut parsed = parse(&new_patch);
            if let Some(file) = parsed.first_mut() {
                file.name = path.to_string_lossy().into_owned();
                file.source = Source::Worktree(full_path);
            }
            files.extend(parsed);
            patch.push_str(&new_patch);
        }
    }
    Ok(Snapshot { patch, files })
}

/// Add lazy previews for tracked and non-ignored untracked files. Changed files
/// keep their patch/index semantics; plain patch loading never calls this.
pub fn add_repository_files(
    snapshot: &mut Snapshot,
    repo: &Path,
    staged: bool,
    paths: &[String],
) -> io::Result<()> {
    let root = repository_root(repo)?;
    let mut command = Command::new("git");
    command
        .current_dir(if paths.is_empty() { &root } else { repo })
        .args(["ls-files", "--cached", "--full-name", "-z"]);
    if !staged {
        command.args(["--others", "--exclude-standard"]);
    }
    let output = command.arg("--").args(paths).output()?;
    if !output.status.success() {
        return Err(io::Error::other("cannot list repository files"));
    }
    let mut seen: HashSet<String> = snapshot.files.iter().map(|f| f.name.clone()).collect();
    for path in git_paths(&output.stdout) {
        let name = path.to_string_lossy().into_owned();
        if !seen.insert(name.clone()) {
            continue;
        }
        let full = root.join(&path);
        if !staged
            && !fs::symlink_metadata(&full).is_ok_and(|m| m.is_file() || m.file_type().is_symlink())
        {
            continue;
        }
        if staged {
            // Gitlinks are directories, not text files. Avoid a recursive index preview.
            if fs::symlink_metadata(&full).is_ok_and(|m| m.is_dir()) {
                continue;
            }
        }
        snapshot.files.push(FileDiff {
            name,
            unchanged: true,
            source: if staged {
                Source::Index {
                    root: root.clone(),
                    path,
                }
            } else {
                Source::Worktree(full)
            },
            ..FileDiff::default()
        });
    }
    Ok(())
}

fn safe_text(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\t' {
                "    ".into()
            } else if c.is_control() {
                "�".into()
            } else {
                c.to_string()
            }
        })
        .collect()
}

pub(crate) fn git_paths(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|bytes| {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                PathBuf::from(OsString::from_vec(bytes.to_vec()))
            }
            #[cfg(not(unix))]
            {
                PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
            }
        })
        .collect()
}

pub(crate) fn repository_root(repo: &Path) -> io::Result<PathBuf> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("cannot locate Git repository root"));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim_end_matches(['\r', '\n']),
    ))
}

fn read_content(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(fs::read_link(path)?.to_string_lossy().as_bytes().to_vec());
    }
    if !metadata.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut file = fs::File::open(path)?;
    let mut probe = vec![0; 8192];
    let read = file.read(&mut probe)?;
    probe.truncate(read);
    if !probe.contains(&0) {
        file.read_to_end(&mut probe)?;
    }
    Ok(probe)
}

fn quoted_path(prefix: &str, path: &Path) -> String {
    let text = format!("{prefix}{}", path.to_string_lossy());
    if text
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        format!(
            "\"{}\"",
            text.chars()
                .map(|c| match c {
                    '\n' => "\\n".into(),
                    '\r' => "\\r".into(),
                    '\t' => "\\t".into(),
                    '"' => "\\\"".into(),
                    '\\' => "\\\\".into(),
                    c if c.is_control() => format!("\\{:03o}", c as u32),
                    c => c.to_string(),
                })
                .collect::<String>()
        )
    } else {
        text
    }
}

fn new_file_patch(path: &Path, bytes: &[u8], metadata: &fs::Metadata) -> String {
    let a = quoted_path("a/", path);
    let b = quoted_path("b/", path);
    let mut mode = "100644";
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            mode = "100755";
        }
    }
    if metadata.file_type().is_symlink() {
        mode = "120000";
    }
    let mut patch = format!("diff --git {a} {b}\nnew file mode {mode}\n");
    if bytes.contains(&0) {
        patch.push_str(&format!("Binary files /dev/null and {b} differ\n"));
        return patch;
    }
    if bytes.is_empty() {
        return patch;
    }
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<_> = text.split_terminator('\n').collect();
    patch.push_str(&format!(
        "--- /dev/null\n+++ {b}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    ));
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    if !bytes.ends_with(b"\n") {
        patch.push_str("\\ No newline at end of file\n");
    }
    patch
}

pub const DEMO: &str = "diff --git a/src/greeting.rs b/src/greeting.rs\nindex 1234567..abcdef0 100644\n--- a/src/greeting.rs\n+++ b/src/greeting.rs\n@@ -1,4 +1,6 @@\n pub fn greet(name: &str) -> String {\n-    format!(\"Hello, {}\", name)\n+    let name = name.trim();\n+    format!(\"Hello, {name}!\")\n }\n+// Ready for a friendlier terminal.\n \ndiff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n-# Greeting\n+# Greeting app\n+Run `cargo run` to get started.\n";

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
    fn repository() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet"]);
        root
    }
    #[test]
    fn repository_catalog_keeps_clean_files_lazy_and_obeys_source_filters_and_ignores() {
        let root = repository();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/clean file.rs"), "clean contents\n").unwrap();
        fs::write(root.path().join("src/binary.bin"), b"image\0bytes").unwrap();
        fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
        git(root.path(), &["add", "."]);
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
                "--quiet",
                "-m",
                "initial",
            ],
        );
        fs::write(root.path().join("ignored.txt"), "private").unwrap();
        let before = git(root.path(), &["ls-files", "--stage", "-z"]);
        let mut snapshot = load(root.path(), false, None, &[]).unwrap();
        assert!(snapshot.files.is_empty());
        add_repository_files(&mut snapshot, root.path(), false, &[]).unwrap();
        assert_eq!(snapshot.files.len(), 3);
        assert!(snapshot
            .files
            .iter()
            .all(|f| f.unchanged && !f.loaded && f.current.is_empty()));
        let clean = snapshot
            .files
            .iter_mut()
            .find(|f| f.name == "src/clean file.rs")
            .unwrap();
        clean.load_current();
        assert_eq!(clean.current[0].text, "clean contents");
        assert!(clean.changes.is_empty());
        let binary = snapshot
            .files
            .iter_mut()
            .find(|f| f.name.ends_with("binary.bin"))
            .unwrap();
        binary.load_current();
        assert!(binary.note.as_deref().unwrap().contains("Binary"));
        fs::write(root.path().join("src/clean file.rs"), "unstaged contents\n").unwrap();
        fs::write(root.path().join("new.txt"), "untracked\n").unwrap();
        let mut staged = load(root.path(), true, None, &[]).unwrap();
        add_repository_files(&mut staged, root.path(), true, &[]).unwrap();
        assert!(!staged.files.iter().any(|f| f.name == "new.txt"));
        let index = staged
            .files
            .iter_mut()
            .find(|f| f.name == "src/clean file.rs")
            .unwrap();
        index.load_current();
        assert_eq!(index.current[0].text, "clean contents");
        let mut working = load(root.path(), false, None, &[]).unwrap();
        let patch = working.patch.clone();
        add_repository_files(&mut working, root.path(), false, &[]).unwrap();
        assert_eq!(working.patch, patch);
        assert_eq!(
            working
                .files
                .iter()
                .filter(|f| f.name == "src/clean file.rs")
                .count(),
            1
        );
        assert!(
            !working
                .files
                .iter()
                .find(|f| f.name == "src/clean file.rs")
                .unwrap()
                .unchanged
        );
        assert!(working.files.iter().any(|f| f.name == "new.txt"));
        let mut filtered = load(
            &root.path().join("src"),
            false,
            None,
            &["binary.bin".into()],
        )
        .unwrap();
        add_repository_files(
            &mut filtered,
            &root.path().join("src"),
            false,
            &["binary.bin".into()],
        )
        .unwrap();
        assert_eq!(filtered.files.len(), 1);
        assert_eq!(filtered.files[0].name, "src/binary.bin");
        assert_eq!(before, git(root.path(), &["ls-files", "--stage", "-z"]));
    }

    #[test]
    fn new_files_respect_ignores_filters_and_leave_index_untouched() {
        let root = repository();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(
            root.path().join(".git/info/exclude"),
            "ignored.txt\n/.tdv.toml\n",
        )
        .unwrap();
        fs::write(root.path().join("src/new file.rs"), "first\nsecond").unwrap();
        fs::write(root.path().join("src/empty.rs"), "").unwrap();
        fs::write(root.path().join("src/binary.dat"), b"binary\0data").unwrap();
        fs::write(root.path().join("ignored.txt"), "private").unwrap();
        fs::write(root.path().join(".tdv.toml"), "private config").unwrap();
        let before = git(root.path(), &["ls-files", "--stage", "-z"]);
        let snapshot = load(&root.path().join("src"), false, None, &[]).unwrap();
        assert_eq!(snapshot.files.len(), 3);
        assert!(snapshot.files.iter().all(|f| f.is_new));
        assert!(!snapshot.patch.contains("private"));
        let mut file = snapshot
            .files
            .into_iter()
            .find(|f| f.name == "src/new file.rs")
            .unwrap();
        file.load_current();
        assert_eq!(file.current.len(), 2);
        assert_eq!(file.counts(), (2, 0));
        assert_eq!(file.change_at(0).unwrap().kind, ChangeKind::Added);
        assert!(file.change_at(0).unwrap().previous.is_empty());
        let filtered = load(root.path(), false, None, &["src/new file.rs".into()]).unwrap();
        assert_eq!(filtered.files.len(), 1);
        assert!(filtered.patch.contains("\\ No newline at end of file"));
        assert_eq!(before, git(root.path(), &["ls-files", "--stage", "-z"]));
        assert!(load(root.path(), true, None, &[]).unwrap().files.is_empty());
    }
    #[test]
    fn full_current_file_keeps_context_outside_the_patch() {
        let root = repository();
        let original: String = (1..=40).map(|n| format!("unchanged {n}\n")).collect();
        fs::write(root.path().join("code.rs"), &original).unwrap();
        git(root.path(), &["add", "code.rs"]);
        fs::write(
            root.path().join("code.rs"),
            original.replace("unchanged 20", "replacement 20"),
        )
        .unwrap();
        let mut snapshot = load(root.path(), false, None, &[]).unwrap();
        assert!(!snapshot.patch.contains("unchanged 1\n"));
        let file = &mut snapshot.files[0];
        file.load_current();
        assert_eq!(file.current.len(), 40);
        assert_eq!(file.current[0].text, "unchanged 1");
        assert_eq!(file.current[39].text, "unchanged 40");
        assert_eq!(file.current[19].text, "replacement 20");
        let change = file.change_at(19).unwrap();
        assert_eq!(change.kind, ChangeKind::Modified);
        assert!(change
            .previous
            .iter()
            .any(|l| l.kind == Kind::Removed && l.text == "unchanged 20"));
        assert!(file.change_at(0).is_none());
    }
    #[test]
    fn staged_current_file_uses_index_instead_of_worktree() {
        let root = repository();
        fs::write(root.path().join("new.rs"), "staged content\n").unwrap();
        git(root.path(), &["add", "new.rs"]);
        fs::write(root.path().join("new.rs"), "unstaged content\n").unwrap();
        let mut snapshot = load(root.path(), true, None, &[]).unwrap();
        let file = &mut snapshot.files[0];
        file.load_current();
        assert!(file.is_new);
        assert_eq!(file.current[0].text, "staged content");
        assert_eq!(file.change_at(0).unwrap().kind, ChangeKind::Added);
    }
    #[test]
    fn deletions_have_current_line_markers_and_previous_code() {
        let root = repository();
        fs::write(root.path().join("code.rs"), "keep\nremove\nlast\n").unwrap();
        git(root.path(), &["add", "code.rs"]);
        fs::write(root.path().join("code.rs"), "keep\nlast\n").unwrap();
        let mut file = load(root.path(), false, None, &[]).unwrap().files.remove(0);
        file.load_current();
        assert_eq!(file.current.len(), 2);
        assert_eq!(file.change_at(1).unwrap().kind, ChangeKind::Removed);
        assert!(file
            .change_at(1)
            .unwrap()
            .previous
            .iter()
            .any(|l| l.text == "remove"));
        fs::remove_file(root.path().join("code.rs")).unwrap();
        let mut file = load(root.path(), false, None, &[]).unwrap().files.remove(0);
        file.load_current();
        assert!(file.current.is_empty());
        assert!(file.is_deleted);
        assert_eq!(file.change_at(0).unwrap().previous.len(), 3);
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_show_the_link_target_without_reading_external_contents() {
        let root = repository();
        let external = tempfile::NamedTempFile::new().unwrap();
        fs::write(external.path(), "external secret contents").unwrap();
        std::os::unix::fs::symlink(external.path(), root.path().join("link")).unwrap();
        let mut snapshot = load(root.path(), false, None, &[]).unwrap();
        assert!(snapshot.patch.contains("new file mode 120000"));
        assert!(!snapshot.patch.contains("external secret contents"));
        snapshot.files[0].load_current();
        assert_eq!(
            snapshot.files[0].current[0].text,
            external.path().to_string_lossy()
        );
    }
    #[test]
    fn numbers_and_alignment() {
        let files = parse(DEMO);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "src/greeting.rs");
        assert_eq!(files[0].counts(), (3, 1));
        let rows = files[0].rows(true);
        let pair = rows
            .iter()
            .find(|(a, _)| a.is_some_and(|l| l.kind == Kind::Removed))
            .unwrap();
        assert_eq!(pair.0.unwrap().old, Some(2));
        assert_eq!(pair.1.unwrap().new, Some(2));
        assert!(rows.iter().any(|(a, b)| a.is_none() && b.is_some()));
    }
    #[test]
    fn empty_binary_deleted_and_escape_content() {
        assert!(parse("").is_empty());
        let f = parse("diff --git a/a b/a\n--- a/a\n+++ /dev/null\n@@ -1 +0,0 @@\n-\u{1b}[31mhi\n");
        assert_eq!(f[0].name, "a");
        assert!(!f[0].lines.last().unwrap().text.contains('\u{1b}'));
        let f = parse("diff --git a/a b/a\nBinary files a/a and b/a differ\n");
        assert!(f[0].lines[0].text.contains("Binary"));
    }
}
