//! Startup checks and consented installs run outside the terminal event loop.
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

const REPOSITORY: &str = "https://github.com/baselsoftwaredev/terminal-diff-viewer";
const LATEST: &str =
    "https://api.github.com/repos/baselsoftwaredev/terminal-diff-viewer/releases/latest";
const MAX_ARCHIVE: u64 = 32 * 1024 * 1024;
const MAX_BINARY: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub asset: String,
}
#[derive(Deserialize)]
struct Metadata {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
}

pub enum Message {
    Checked(Option<Release>),
    Installed(String),
    Failed(String),
}

pub struct Updater {
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    started: bool,
    busy: bool,
    enabled: bool,
}

impl Updater {
    pub fn new(enabled: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            started: false,
            busy: false,
            enabled,
        }
    }
    /// Called only after the first frame. No network client is created on the UI thread.
    pub fn start_check(&mut self) {
        if self.started || !self.enabled {
            return;
        }
        self.started = true;
        let tx = self.sender.clone();
        self.busy = thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                // Offline, rate-limited, and unavailable checks never interrupt code viewing.
                let release = check().ok().flatten();
                let _ = tx.send(Message::Checked(release));
            })
            .is_ok();
    }
    pub fn install(&mut self, release: Release) {
        let tx = self.sender.clone();
        self.busy = thread::Builder::new()
            .name("update-install".into())
            .spawn(move || {
                let message = match install(&release) {
                    Ok(()) => Message::Installed(release.tag),
                    Err(err) => Message::Failed(err.to_string()),
                };
                let _ = tx.send(message);
            })
            .is_ok();
    }
    pub fn receive(&mut self) -> Option<Message> {
        let message = self.receiver.try_recv().ok()?;
        self.busy = false;
        Some(message)
    }
    pub fn is_busy(&self) -> bool {
        self.busy
    }
}

fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn archive_name(target: &str) -> String {
    format!(
        "tdv-{target}.{}",
        if target.contains("windows") {
            "zip"
        } else {
            "tar.gz"
        }
    )
}

fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(seconds)))
        .build()
        .into()
}

fn fetch(agent: &ureq::Agent, url: &str, max: u64) -> io::Result<Vec<u8>> {
    let mut response = agent
        .get(url)
        .header("User-Agent", concat!("tdv/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(io::Error::other)?;
    response
        .body_mut()
        .with_config()
        .limit(max)
        .read_to_vec()
        .map_err(io::Error::other)
}

fn parse_release(bytes: &[u8], current: &str, target: &str) -> io::Result<Option<Release>> {
    let metadata: Metadata = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    let tag = metadata.tag_name;
    // Canonical SemVer tags also ensure the download path contains no URL metacharacters.
    let Some(version) = tag.strip_prefix('v').and_then(|s| Version::parse(s).ok()) else {
        return Ok(None);
    };
    let current = Version::parse(current).map_err(io::Error::other)?;
    let asset = archive_name(target);
    if metadata.draft
        || metadata.prerelease
        || !version.pre.is_empty()
        || !version.build.is_empty()
        || version <= current
        || !metadata.assets.iter().any(|a| a.name == asset)
        || !metadata.assets.iter().any(|a| a.name == "SHA256SUMS")
    {
        return Ok(None);
    }
    Ok(Some(Release { tag, asset }))
}

fn check() -> io::Result<Option<Release>> {
    let Some(target) = platform() else {
        return Ok(None);
    };
    parse_release(
        &fetch(&agent(5), LATEST, 1024 * 1024)?,
        env!("CARGO_PKG_VERSION"),
        target,
    )
}

fn verify(archive: &[u8], manifest: &[u8], asset: &str) -> io::Result<()> {
    let manifest = std::str::from_utf8(manifest).map_err(io::Error::other)?;
    let hashes: Vec<_> = manifest
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            (fields.next()? == asset).then_some(hash)
        })
        .collect();
    let actual = format!("{:x}", Sha256::digest(archive));
    if hashes.len() != 1 || !actual.eq_ignore_ascii_case(hashes[0]) {
        return Err(io::Error::other(
            "release checksum verification failed; current version was kept",
        ));
    }
    Ok(())
}

fn write_binary(mut reader: impl Read, destination: &Path) -> io::Result<()> {
    let mut file = fs::File::create(destination)?;
    let written = io::copy(&mut reader.by_ref().take(MAX_BINARY + 1), &mut file)?;
    if written == 0 || written > MAX_BINARY {
        return Err(io::Error::other("invalid release binary size"));
    }
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn extract(bytes: &[u8], destination: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.path()?.as_ref() == Path::new("tdv") {
            if !entry.header().entry_type().is_file() {
                return Err(io::Error::other("release binary is not a regular file"));
            }
            return write_binary(entry, destination);
        }
    }
    Err(io::Error::other("release archive has no tdv binary"))
}

#[cfg(windows)]
fn extract(bytes: &[u8], destination: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes)).map_err(io::Error::other)?;
    let entry = archive.by_name("tdv.exe").map_err(io::Error::other)?;
    write_binary(entry, destination)
}

fn install_with(
    release: &Release,
    download: impl Fn(&str, u64) -> io::Result<Vec<u8>>,
    replace: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let base = format!("{REPOSITORY}/releases/download/{}", release.tag);
    let bytes = download(&format!("{base}/{}", release.asset), MAX_ARCHIVE)?;
    let manifest = download(&format!("{base}/SHA256SUMS"), 64 * 1024)?;
    verify(&bytes, &manifest, &release.asset)?;
    let directory = tempfile::Builder::new().prefix("tdv-update-").tempdir()?;
    let binary = directory
        .path()
        .join(if cfg!(windows) { "tdv.exe" } else { "tdv" });
    extract(&bytes, &binary)?;
    replace(&binary).map_err(|err| {
        io::Error::other(format!(
            "could not replace tdv: {err}; install it in a user-writable directory and retry"
        ))
    })
}

fn install(release: &Release) -> io::Result<()> {
    let client = agent(90);
    install_with(
        release,
        |url, max| fetch(&client, url, max),
        |path| self_replace::self_replace(path),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(tag: &str, draft: bool, prerelease: bool) -> Vec<u8> {
        serde_json::to_vec(
            &serde_json::json!({"tag_name": tag, "draft": draft, "prerelease": prerelease,
            "assets": [{"name": "tdv-aarch64-apple-darwin.tar.gz"}, {"name": "SHA256SUMS"}]}),
        )
        .unwrap()
    }
    #[test]
    fn offers_only_complete_newer_stable_releases() {
        let target = "aarch64-apple-darwin";
        assert!(
            parse_release(&metadata("v0.3.0", false, false), "0.2.0", target)
                .unwrap()
                .is_some()
        );
        for (tag, draft, prerelease) in [
            ("v0.2.0", false, false),
            ("v0.1.0", false, false),
            ("v0.3.0", true, false),
            ("v0.3.0", false, true),
            ("v0.3.0-beta.1", false, false),
            ("../../bad", false, false),
        ] {
            assert!(
                parse_release(&metadata(tag, draft, prerelease), "0.2.0", target)
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            parse_release(&metadata("v0.3.0", false, false), "0.2.0", "unknown")
                .unwrap()
                .is_none()
        );
        assert!(parse_release(b"not json", "0.2.0", target).is_err());
    }
    #[test]
    fn checksum_failure_never_reaches_replacement() {
        let release = Release {
            tag: "v0.3.0".into(),
            asset: "tdv-test.tar.gz".into(),
        };
        let result = install_with(
            &release,
            |_, _| Ok(b"invalid data".to_vec()),
            |_| panic!("must not replace on checksum failure"),
        );
        assert!(result.is_err());
        let bytes = b"archive";
        let manifest = format!("{:x}  tdv-test.tar.gz\n", Sha256::digest(bytes));
        assert!(verify(bytes, manifest.as_bytes(), &release.asset).is_ok());
        assert!(verify(b"tampered", manifest.as_bytes(), &release.asset).is_err());
        assert!(verify(bytes, manifest.repeat(2).as_bytes(), &release.asset).is_err());
    }
    #[test]
    fn disabled_check_starts_no_worker() {
        let mut updater = Updater::new(false);
        updater.start_check();
        assert!(!updater.started);
        assert!(!updater.is_busy());
        assert!(updater.receive().is_none());
    }
    fn archive_fixture(binary: &[u8]) -> Vec<u8> {
        #[cfg(not(windows))]
        {
            let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            let mut archive = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(binary.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, "tdv", binary).unwrap();
            archive.into_inner().unwrap().finish().unwrap()
        }
        #[cfg(windows)]
        {
            let mut archive = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
            archive
                .start_file("tdv.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(binary).unwrap();
            archive.finish().unwrap().into_inner()
        }
    }
    #[test]
    fn verified_binary_is_extracted_before_replacement() {
        let binary = b"test executable bytes";
        let bytes = archive_fixture(binary);
        let release = Release {
            tag: "v0.3.0".into(),
            asset: archive_name(platform().unwrap()),
        };
        let manifest = format!("{:x}  {}\n", Sha256::digest(&bytes), release.asset);
        let replaced = std::cell::Cell::new(false);
        install_with(
            &release,
            |url, _| {
                assert!(url.starts_with(&format!("{REPOSITORY}/releases/download/v0.3.0/")));
                Ok(if url.ends_with("SHA256SUMS") {
                    manifest.as_bytes().to_vec()
                } else {
                    bytes.clone()
                })
            },
            |path| {
                assert_eq!(fs::read(path)?, binary);
                replaced.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(replaced.get());
        let result = install_with(
            &release,
            |url, _| {
                Ok(if url.ends_with("SHA256SUMS") {
                    manifest.as_bytes().to_vec()
                } else {
                    bytes.clone()
                })
            },
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "read-only installation",
                ))
            },
        );
        assert!(result.unwrap_err().to_string().contains("user-writable"));
    }
}
