//! Coalesced background reloads. Git status and file metadata are polled once a
//! second; full diffs are rebuilt only after a change or an explicit refresh.
use crate::diff::{self, Snapshot};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    process::Command as Process,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::Duration,
};

const INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct Parameters {
    pub repo: PathBuf,
    pub staged: bool,
    pub base: Option<String>,
    pub paths: Vec<String>,
    pub demo: bool,
}
impl Parameters {
    fn load(&self) -> io::Result<Snapshot> {
        let mut snapshot = if self.demo {
            Snapshot::from_patch(diff::DEMO)
        } else {
            diff::load(&self.repo, self.staged, self.base.as_deref(), &self.paths)?
        };
        // No filesystem or Git reads are needed when the UI accepts this snapshot.
        for file in &mut snapshot.files {
            file.load_current();
        }
        Ok(snapshot)
    }
}

struct Request {
    generation: u64,
    params: Parameters,
    auto: bool,
}
pub struct Reply {
    pub snapshot: io::Result<Snapshot>,
}
struct ResultMessage {
    generation: u64,
    snapshot: io::Result<Snapshot>,
}
struct Active {
    request: Request,
    probe: Option<Probe>,
    signature: Option<u64>,
    snapshot_hash: Option<u64>,
}

pub struct Refresher {
    requests: SyncSender<Request>,
    replies: Receiver<ResultMessage>,
    pending: Option<Request>,
    generation: u64,
    busy: bool,
    auto: bool,
    pub started: bool,
}

impl Refresher {
    pub fn new() -> Self {
        let (requests, rx) = mpsc::sync_channel::<Request>(1);
        let (tx, replies) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("diff-refresh".into())
            .spawn(move || {
                let mut active: Option<Active> = None;
                loop {
                    let request = if active.as_ref().is_some_and(|active| active.request.auto) {
                        rx.recv_timeout(INTERVAL)
                    } else {
                        rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    match request {
                        Ok(mut request) => {
                            while let Ok(latest) = rx.try_recv() {
                                request = latest;
                            }
                            let probe = if request.params.demo {
                                None
                            } else {
                                Probe::new(&request.params.repo).ok()
                            };
                            let signature =
                                probe.as_ref().and_then(|probe| probe.fingerprint().ok());
                            let snapshot = request.params.load();
                            let snapshot_hash = snapshot.as_ref().ok().map(snapshot_digest);
                            if tx
                                .send(ResultMessage {
                                    generation: request.generation,
                                    snapshot,
                                })
                                .is_err()
                            {
                                break;
                            }
                            active = Some(Active {
                                request,
                                probe,
                                signature,
                                snapshot_hash,
                            });
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let Some(active) = &mut active else {
                                continue;
                            };
                            if active.probe.is_none() {
                                active.probe = Probe::new(&active.request.params.repo).ok();
                            }
                            let latest = active
                                .probe
                                .as_ref()
                                .and_then(|probe| probe.fingerprint().ok());
                            if latest.is_none() || latest != active.signature {
                                let snapshot = active.request.params.load();
                                active.signature = latest;
                                let digest = snapshot.as_ref().ok().map(snapshot_digest);
                                if digest.is_some() && digest == active.snapshot_hash {
                                    continue;
                                }
                                active.snapshot_hash = digest;
                                if tx
                                    .send(ResultMessage {
                                        generation: active.request.generation,
                                        snapshot,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        Self {
            requests,
            replies,
            pending: None,
            generation: 0,
            busy: false,
            auto: false,
            started: false,
        }
    }
    pub fn request(&mut self, params: Parameters, auto: bool) {
        self.generation = self.generation.wrapping_add(1);
        self.auto = auto && !params.demo;
        self.started = true;
        self.busy = true;
        self.pending = Some(Request {
            generation: self.generation,
            params,
            auto: self.auto,
        });
        self.flush();
    }
    pub fn flush(&mut self) {
        let Some(request) = self.pending.take() else {
            return;
        };
        match self.requests.try_send(request) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(request)) => self.pending = Some(request),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.busy = false;
                self.auto = false;
            }
        }
    }
    pub fn receive(&mut self) -> Option<Reply> {
        let mut latest = None;
        while let Ok(reply) = self.replies.try_recv() {
            if reply.generation == self.generation {
                self.busy = false;
                latest = Some(Reply {
                    snapshot: reply.snapshot,
                });
            }
        }
        latest
    }
    pub fn is_active(&self) -> bool {
        self.busy || self.auto || self.pending.is_some()
    }
}

fn snapshot_digest(snapshot: &Snapshot) -> u64 {
    let mut hash = DefaultHasher::new();
    snapshot.patch.hash(&mut hash);
    for file in &snapshot.files {
        file.name.hash(&mut hash);
        file.note.hash(&mut hash);
        for line in &file.current {
            line.text.hash(&mut hash);
        }
    }
    hash.finish()
}

struct Probe {
    root: PathBuf,
    index: PathBuf,
}
impl Probe {
    fn new(repo: &Path) -> io::Result<Self> {
        let root = diff::repository_root(repo)?;
        let output = Process::new("git")
            .current_dir(&root)
            .args(["rev-parse", "--git-path", "index"])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other("cannot find Git index"));
        }
        let index =
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim_end_matches(['\r', '\n']));
        let index = if index.is_absolute() {
            index
        } else {
            root.join(index)
        };
        Ok(Self { root, index })
    }
    fn fingerprint(&self) -> io::Result<u64> {
        let output = Process::new("git")
            .current_dir(&self.root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args([
                "status",
                "--porcelain=v2",
                "--branch",
                "--no-renames",
                "-z",
                "--untracked-files=all",
            ])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let mut hash = DefaultHasher::new();
        output.stdout.hash(&mut hash);
        hash_metadata(&self.index, &mut hash);
        for record in output.stdout.split(|b| *b == 0) {
            let path = match record.first() {
                Some(b'?') => record.get(2..),
                Some(b'1') => record.splitn(9, |b| *b == b' ').nth(8),
                Some(b'u') => record.splitn(11, |b| *b == b' ').nth(10),
                _ => None,
            };
            if let Some(path) = path.and_then(|bytes| diff::git_paths(bytes).into_iter().next()) {
                hash_metadata(&self.root.join(path), &mut hash);
            }
        }
        Ok(hash.finish())
    }
}

fn hash_metadata(path: &Path, hash: &mut DefaultHasher) {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            metadata.len().hash(hash);
            metadata.modified().ok().hash(hash);
            metadata.created().ok().hash(hash);
            metadata.permissions().readonly().hash(hash);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                metadata.ino().hash(hash);
                metadata.ctime().hash(hash);
                metadata.ctime_nsec().hash(hash);
            }
        }
        Err(_) => false.hash(hash),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_queue_coalesces_requests_and_stale_source_results_are_rejected() {
        let (requests, rx) = mpsc::sync_channel(1);
        let (tx, replies) = mpsc::channel();
        let mut refresher = Refresher {
            requests,
            replies,
            pending: None,
            generation: 0,
            busy: false,
            auto: false,
            started: false,
        };
        let params = Parameters {
            repo: PathBuf::new(),
            staged: false,
            base: None,
            paths: vec![],
            demo: true,
        };
        refresher.request(params.clone(), false);
        refresher.request(
            Parameters {
                staged: true,
                ..params
            },
            false,
        );
        tx.send(ResultMessage {
            generation: 1,
            snapshot: Ok(Snapshot::from_patch(diff::DEMO)),
        })
        .unwrap();
        assert!(refresher.receive().is_none());
        assert!(refresher.is_active());
        assert!(!rx.try_recv().unwrap().params.staged);
        refresher.flush();
        let latest = rx.try_recv().unwrap();
        assert!(latest.params.staged);
        tx.send(ResultMessage {
            generation: latest.generation,
            snapshot: Ok(Snapshot::from_patch(diff::DEMO)),
        })
        .unwrap();
        assert!(refresher.receive().is_some());
        assert!(!refresher.is_active());
    }
    fn git(root: &Path, args: &[&str]) {
        assert!(Process::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    #[test]
    fn detects_repeated_edits_new_files_staging_and_ignores() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet"]);
        fs::write(root.path().join(".git/info/exclude"), "ignored.txt\n").unwrap();
        fs::write(root.path().join("file.rs"), "original").unwrap();
        git(root.path(), &["add", "file.rs"]);
        let probe = Probe::new(root.path()).unwrap();
        let first = probe.fingerprint().unwrap();
        fs::write(root.path().join("ignored.txt"), "ignored").unwrap();
        assert_eq!(first, probe.fingerprint().unwrap());
        fs::write(root.path().join("file.rs"), "change one").unwrap();
        let second = probe.fingerprint().unwrap();
        assert_ne!(first, second);
        fs::write(
            root.path().join("file.rs"),
            "change two with different size",
        )
        .unwrap();
        let third = probe.fingerprint().unwrap();
        assert_ne!(second, third);
        thread::sleep(Duration::from_millis(25));
        fs::write(
            root.path().join("file.rs"),
            "change NEW with different size",
        )
        .unwrap();
        assert_ne!(third, probe.fingerprint().unwrap());
        git(root.path(), &["add", "file.rs"]);
        let staged = probe.fingerprint().unwrap();
        assert_ne!(third, staged);
        fs::write(root.path().join("new.rs"), "new").unwrap();
        assert_ne!(staged, probe.fingerprint().unwrap());
    }
    #[test]
    fn manual_refresh_delivers_hydrated_data_without_waiting_on_ui() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet"]);
        fs::write(root.path().join("new.rs"), "new code\n").unwrap();
        let mut refresher = Refresher::new();
        refresher.request(
            Parameters {
                repo: root.path().into(),
                staged: false,
                base: None,
                paths: vec![],
                demo: false,
            },
            false,
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            if let Some(reply) = refresher.receive() {
                break reply.snapshot.unwrap();
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(snapshot.files[0].current[0].text, "new code");
        assert!(!refresher.is_active());
    }
}
