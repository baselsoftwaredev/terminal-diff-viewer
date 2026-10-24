use std::{io, path::Path, process::Command};

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
}

impl FileDiff {
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
            });
            in_hunk = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if !in_hunk && (text.starts_with("--- ") || text.starts_with("+++ ")) {
            let path = &text[4..];
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
        let safe = content
            .chars()
            .map(|c| {
                if c == '\t' {
                    "    ".to_owned()
                } else if c.is_control() {
                    "�".to_owned()
                } else {
                    c.to_string()
                }
            })
            .collect();
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

pub fn load(repo: &Path, staged: bool, base: Option<&str>, paths: &[String]) -> io::Result<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo).args([
        "--no-pager",
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--patch",
        "--unified=3",
    ]);
    if staged {
        cmd.arg("--cached");
    }
    if let Some(base) = base {
        if base.starts_with('-') {
            return Err(io::Error::other("base revision cannot start with '-'"));
        }
        cmd.arg(base);
    }
    let output = cmd.arg("--").args(paths).output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub const DEMO: &str = "diff --git a/src/greeting.rs b/src/greeting.rs\nindex 1234567..abcdef0 100644\n--- a/src/greeting.rs\n+++ b/src/greeting.rs\n@@ -1,4 +1,6 @@\n pub fn greet(name: &str) -> String {\n-    format!(\"Hello, {}\", name)\n+    let name = name.trim();\n+    format!(\"Hello, {name}!\")\n }\n+// Ready for a friendlier terminal.\n \ndiff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n-# Greeting\n+# Greeting app\n+Run `cargo run` to get started.\n";

#[cfg(test)]
mod tests {
    use super::*;
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
