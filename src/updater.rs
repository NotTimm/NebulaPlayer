//! In-app updates for the installs that can replace themselves: the Windows
//! installer (runs the new setup silently), the Windows portable zip (swaps
//! the files next to the exe) and the Mac app (swaps the .app bundle).
//! Everything else updates through its own channel; see
//! `updates::Install::how_to_update`.
//!
//! Every release file has a minisign signature next to it
//! (`<name>.minisig`), made by the release workflow with the project's
//! secret key. Nothing is installed unless that signature checks out against
//! `PUBLIC_KEY` and its trusted comment is `file:<name>`, so an older signed
//! file can't be passed off as a newer one.
//!
//! An update runs in two steps: `start` downloads, checks and unpacks the new
//! version next to the old one (nothing is replaced yet), then `apply`, just
//! before the app quits, puts it in place and starts the new version.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::updates::{Asset, Install, Release};

/// The key releases are signed with (minisign key ID AFE5BA3AD81D4B31). Its
/// secret half is only in the owner's keyring and the repository's
/// `MINISIGN_KEY` secret. Changing it takes a release signed with the old key
/// that ships the new one.
pub const PUBLIC_KEY: &str = "RWQxSx3YOrrlrxqPlxFEY6QXstC2sjcXdOCKJNHI1xtIKT1o8nN41ZIV";

/// Downloads bigger than this are refused (the packages are ~60 MB,
/// with mpv).
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
/// Where the portable zip's new files wait, inside its folder.
const PORTABLE_STAGING: &str = ".nebula-update";
/// What files replaced in the portable folder are renamed to until the next
/// start deletes them (Windows can't delete a running exe, but can rename it).
const OLD_SUFFIX: &str = ".nebula-old";
/// Prefix of the folder next to the Mac app that the new bundle unpacks into.
const MAC_STAGING_PREFIX: &str = ".nebula-update-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    WindowsInstaller,
    WindowsPortable,
    MacApp,
}

impl Kind {
    pub fn for_install(install: Install) -> Option<Self> {
        match install {
            Install::WindowsInstaller => Some(Kind::WindowsInstaller),
            Install::WindowsPortable => Some(Kind::WindowsPortable),
            // Only Apple Silicon builds are published.
            Install::MacApp if cfg!(target_arch = "aarch64") => Some(Kind::MacApp),
            _ => None,
        }
    }

    fn asset_suffix(self) -> &'static str {
        match self {
            Kind::WindowsInstaller => "-windows-x64-setup.exe",
            Kind::WindowsPortable => "-windows-x64.zip",
            Kind::MacApp => "-macos-arm64.app.tar.gz",
        }
    }
}

/// What to download for an update, and where this copy of the app lives.
#[derive(Clone, Debug)]
pub struct Plan {
    pub kind: Kind,
    pub version: String,
    pub file: Asset,
    pub signature: Asset,
    /// The running exe.
    pub exe: PathBuf,
    /// The folder with the exe (Windows) or the .app bundle (Mac).
    pub app: PathBuf,
}

impl Plan {
    /// The update for this copy, when it can update itself and the release
    /// has a signed package for it. Releases before 0.1.7 have no
    /// signatures, so they're never installed this way.
    pub fn for_release(install: Install, release: &Release) -> Option<Plan> {
        let kind = Kind::for_install(install)?;
        let exe = std::env::current_exe().ok()?;
        let app = match kind {
            Kind::WindowsInstaller | Kind::WindowsPortable => exe.parent()?.to_path_buf(),
            // …/Nebula Player.app/Contents/MacOS/nebula-player
            Kind::MacApp => exe.ancestors().nth(3)?.to_path_buf(),
        };
        Plan::new(kind, release, exe, app)
    }

    fn new(kind: Kind, release: &Release, exe: PathBuf, app: PathBuf) -> Option<Plan> {
        let file = release
            .assets
            .iter()
            .find(|a| a.name.ends_with(kind.asset_suffix()) && a.name.contains(&release.version))?
            .clone();
        let signature_name = format!("{}.minisig", file.name);
        let signature = release.assets.iter().find(|a| a.name == signature_name)?.clone();
        Some(Plan { kind, version: release.version.clone(), file, signature, exe, app })
    }
}

/// Checks that `signature` (a .minisig file's text) is `public_key`'s
/// signature of `data`, made for a file called `file_name`.
pub fn verify(public_key: &str, data: &[u8], signature: &str, file_name: &str) -> Result<(), String> {
    let key = minisign_verify::PublicKey::from_base64(public_key).map_err(|e| format!("bad update key: {e}"))?;
    let sig = minisign_verify::Signature::decode(signature).map_err(|e| format!("unreadable signature: {e}"))?;
    key.verify(data, &sig, false)
        .map_err(|_| "the download isn't signed with Nebula Player's release key".to_string())?;
    if sig.trusted_comment() != format!("file:{file_name}") {
        return Err(format!("the signature is for another file ({})", sig.trusted_comment()));
    }
    Ok(())
}

/// How a running update is getting on.
#[derive(Clone, Debug)]
pub enum Progress {
    Downloading { done: u64, total: Option<u64> },
    /// Downloaded, checked and unpacked; `apply` puts it in place.
    Ready(Staged),
    Failed(String),
}

/// A checked new version waiting next to the old one.
#[derive(Clone, Debug)]
pub struct Staged {
    pub plan: Plan,
    /// The setup .exe, the folder with the zip's files, or the new .app.
    pub path: PathBuf,
}

/// Downloads, checks and unpacks `plan`'s package, reporting as it goes.
pub fn start(plan: Plan) -> impl cosmic::iced::futures::Stream<Item = Progress> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::stream::channel(8, async move |mut tx: cosmic::iced::futures::channel::mpsc::Sender<Progress>| {
        let mut report = tx.clone();
        let result = fetch_and_stage(plan, PUBLIC_KEY, move |done, total| {
            // Progress may be dropped when the UI is behind; the end isn't.
            let _ = report.try_send(Progress::Downloading { done, total });
        })
        .await;
        let _ = tx
            .send(match result {
                Ok(staged) => Progress::Ready(staged),
                Err(e) => {
                    log::warn!("update: {e}");
                    Progress::Failed(e)
                }
            })
            .await;
    })
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            // No overall timeout: a slow connection may take minutes.
            .connect_timeout(std::time::Duration::from_secs(20))
            .read_timeout(std::time::Duration::from_secs(60))
            .user_agent(concat!("nebula-player/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client")
    })
}

async fn fetch_and_stage(
    plan: Plan,
    public_key: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Staged, String> {
    if plan.kind == Kind::MacApp {
        check_mac_location(&plan.app)?;
    }
    let signature = client()
        .get(&plan.signature.url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("couldn't download the signature: {e}"))?
        .text()
        .await
        .map_err(|e| format!("couldn't download the signature: {e}"))?;

    let mut resp = client()
        .get(&plan.file.url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("couldn't download the update: {e}"))?;
    let total = resp.content_length();
    if total.is_some_and(|t| t > MAX_DOWNLOAD) {
        return Err("the update is unexpectedly large".into());
    }
    let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("the download was cut off: {e}"))? {
        data.extend_from_slice(&chunk);
        if data.len() as u64 > MAX_DOWNLOAD {
            return Err("the update is unexpectedly large".into());
        }
        progress(data.len() as u64, total);
    }

    verify(public_key, &data, &signature, &plan.file.name)?;

    tokio::task::spawn_blocking(move || {
        let path = stage(&plan, &data)?;
        Ok(Staged { plan, path })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Puts the checked package where `apply` will take it from, without
/// touching the installed app.
fn stage(plan: &Plan, data: &[u8]) -> Result<PathBuf, String> {
    match plan.kind {
        Kind::WindowsInstaller => {
            let dir = std::env::temp_dir().join("NebulaPlayer-update");
            fs::create_dir_all(&dir).map_err(|e| format!("couldn't save the update: {e}"))?;
            let path = dir.join(&plan.file.name);
            fs::write(&path, data).map_err(|e| format!("couldn't save the update: {e}"))?;
            Ok(path)
        }
        Kind::WindowsPortable => {
            let dir = plan.app.join(PORTABLE_STAGING);
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).map_err(|e| format!("can't write to {}: {e}", plan.app.display()))?;
            extract_zip(data, &dir)?;
            let exe_name = plan.exe.file_name().ok_or("no exe name")?;
            if !dir.join(exe_name).is_file() {
                return Err(format!("the update has no {}", exe_name.to_string_lossy()));
            }
            Ok(dir)
        }
        Kind::MacApp => {
            let parent = plan.app.parent().ok_or("the app has no folder")?;
            let dir = parent.join(format!("{MAC_STAGING_PREFIX}{}", plan.version));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir(&dir).map_err(|e| format!("can't write to {}: {e}", parent.display()))?;
            let archive = dir.join(&plan.file.name);
            fs::write(&archive, data).map_err(|e| format!("couldn't save the update: {e}"))?;
            // The system tar keeps the bundle's permissions and symlinks.
            let out = std::process::Command::new("tar")
                .arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(&dir)
                .output()
                .map_err(|e| format!("couldn't run tar: {e}"))?;
            if !out.status.success() {
                return Err(format!("couldn't unpack the update: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            let _ = fs::remove_file(&archive);
            let bundle = fs::read_dir(&dir)
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().is_some_and(|x| x == "app") && p.is_dir())
                .ok_or("the update has no .app")?;
            let exe_name = plan.exe.file_name().ok_or("no exe name")?;
            if !bundle.join("Contents/MacOS").join(exe_name).is_file() {
                return Err("the update's app has no program in it".into());
            }
            Ok(bundle)
        }
    }
}

/// macOS runs an app opened straight from Downloads or the disk image from a
/// read-only copy ("App Translocation"), which can't be replaced.
fn check_mac_location(app: &Path) -> Result<(), String> {
    let s = app.to_string_lossy();
    if s.contains("/AppTranslocation/") || s.starts_with("/Volumes/") {
        return Err("Nebula Player is running from the disk image or Downloads, where it can't update itself. \
                    Drag Nebula Player into Applications, open it from there and try again."
            .into());
    }
    Ok(())
}

/// Unpacks the Windows zip into `dest`, dropping the one top-level folder
/// the release zip has (`NebulaPlayer-<version>-windows-x64/`).
fn extract_zip(data: &[u8], dest: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(io::Cursor::new(data)).map_err(|e| format!("bad zip: {e}"))?;
    let mut entries = Vec::new();
    for i in 0..zip.len() {
        let file = zip.by_index(i).map_err(|e| format!("bad zip: {e}"))?;
        let name = file.name().map_err(|e| format!("bad zip: {e}"))?.replace('\\', "/");
        let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
        if parts.iter().any(|p| *p == ".." || p.contains(':')) || name.starts_with('/') {
            return Err(format!("bad path in zip: {name}"));
        }
        entries.push((i, parts.iter().map(|p| p.to_string()).collect::<Vec<_>>(), file.is_dir()));
    }
    let top = entries.first().and_then(|(_, p, _)| p.first().cloned());
    let strip = top.is_some_and(|top| entries.iter().all(|(_, p, d)| p.first() == Some(&top) && (p.len() > 1 || *d)));
    for (i, parts, is_dir) in entries {
        let parts = if strip { &parts[1..] } else { &parts[..] };
        if parts.is_empty() {
            continue;
        }
        let path = parts.iter().fold(dest.to_path_buf(), |p, part| p.join(part));
        if is_dir {
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file = zip.by_index(i).map_err(|e| format!("bad zip: {e}"))?;
        let mut out = fs::File::create(&path).map_err(|e| e.to_string())?;
        io::copy(&mut file, &mut out).map_err(|e| format!("couldn't unpack {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Moves every file under `from` to the same place under `to`. Files it
/// replaces are renamed to `<name>.nebula-old` first (which works for a
/// running exe on Windows) and deleted at the next start. If a move fails,
/// what was done is undone.
fn swap_in(from: &Path, to: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(from, Path::new(""), &mut files).map_err(|e| e.to_string())?;
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for rel in files {
        let (src, dst) = (from.join(&rel), to.join(&rel));
        match move_one(&src, &dst) {
            Ok(old) => done.push((dst, old)),
            Err(e) => {
                for (dst, old) in done.into_iter().rev() {
                    let _ = fs::remove_file(&dst);
                    if let Some(old) = old {
                        let _ = fs::rename(&old, &dst);
                    }
                }
                return Err(format!("couldn't replace {}: {e}", dst.display()));
            }
        }
    }
    Ok(())
}

fn move_one(src: &Path, dst: &Path) -> io::Result<Option<PathBuf>> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let old = if dst.exists() {
        let old = free_old_name(dst);
        fs::rename(dst, &old)?;
        Some(old)
    } else {
        None
    };
    if let Err(e) = fs::rename(src, dst) {
        if let Some(old) = &old {
            let _ = fs::rename(old, dst);
        }
        return Err(e);
    }
    Ok(old)
}

/// `<name>.nebula-old`, or a numbered one when a leftover can't be deleted
/// (still held by an old copy that hasn't quit yet).
fn free_old_name(path: &Path) -> PathBuf {
    let mut n = 0;
    loop {
        let mut name = path.as_os_str().to_owned();
        name.push(if n == 0 { OLD_SUFFIX.to_string() } else { format!("{OLD_SUFFIX}{n}") });
        let candidate = PathBuf::from(name);
        if !candidate.exists() || fs::remove_file(&candidate).is_ok() || n >= 20 {
            return candidate;
        }
        n += 1;
    }
}

fn collect_files(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root.join(rel))? {
        let entry = entry?;
        let rel = rel.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            collect_files(root, &rel, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// Puts the staged version in place and starts it once this process has
/// quit. The caller quits right after.
pub fn apply(staged: &Staged) -> Result<(), String> {
    let plan = &staged.plan;
    match plan.kind {
        Kind::WindowsInstaller => {
            let log = staged.path.with_extension("log");
            run_after_exit_windows(&installer_script(&staged.path, &log, &plan.exe))
        }
        Kind::WindowsPortable => {
            swap_in(&staged.path, &plan.app)?;
            let _ = fs::remove_dir_all(&staged.path);
            run_after_exit_windows(&relaunch_script(&plan.exe))
                .map_err(|e| format!("Nebula Player was updated but couldn't restart itself ({e}); start it again"))
        }
        Kind::MacApp => {
            let staging = staged.path.parent().ok_or("no staging folder")?;
            swap_bundle(&staged.path, &plan.app, &staging.join("previous.app"))?;
            mac_restart(std::process::id(), &plan.app, staging)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("couldn't restart Nebula Player: {e}"))
        }
    }
}

/// The cmd line that waits a moment for this copy to quit, installs over
/// it (the installer would close it if it hadn't), then starts the new one.
fn installer_script(setup: &Path, log: &Path, exe: &Path) -> String {
    format!(
        "ping -n 3 127.0.0.1 >nul & \"{}\" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS \
         /NORESTARTAPPLICATIONS \"/LOG={}\" & start \"\" \"{}\"",
        setup.display(),
        log.display(),
        exe.display()
    )
}

/// The cmd line that starts the swapped-in exe once this copy has quit.
fn relaunch_script(exe: &Path) -> String {
    format!("ping -n 2 127.0.0.1 >nul & start \"\" \"{}\"", exe.display())
}

/// A shell that waits for process `pid` to quit (`open` would otherwise just
/// bring it to the front), clears `staging` away and opens `app`.
fn mac_restart(pid: u32, app: &Path, staging: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; rm -rf "$3"; open "$2""#)
        .arg("nebula-update")
        .arg(pid.to_string())
        .arg(app)
        .arg(staging)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Moves the bundle at `app` to `old` and the new one at `new` into its
/// place, putting the old one back if that fails.
fn swap_bundle(new: &Path, app: &Path, old: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(old);
    fs::rename(app, old).map_err(|e| format!("couldn't move the old app aside: {e}"))?;
    if let Err(e) = fs::rename(new, app) {
        let _ = fs::rename(old, app);
        return Err(format!("couldn't put the new app in place: {e}"));
    }
    Ok(())
}

#[cfg(windows)]
fn run_after_exit_windows(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // cmd strips the outer quotes of what follows /C and keeps the rest.
    std::process::Command::new("cmd")
        .raw_arg("/C")
        .raw_arg(format!("\"{script}\""))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("couldn't start the installer: {e}"))
}

#[cfg(not(windows))]
fn run_after_exit_windows(_script: &str) -> Result<(), String> {
    Err("not on Windows".into())
}

/// Deletes what an earlier update left behind: replaced files of the
/// portable zip, an unused staging folder, the downloaded installer.
pub fn clean_up() {
    let Some(kind) = Kind::for_install(Install::detect()) else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    match kind {
        Kind::WindowsInstaller => {
            let _ = fs::remove_dir_all(std::env::temp_dir().join("NebulaPlayer-update"));
        }
        Kind::WindowsPortable => {
            if let Some(dir) = exe.parent() {
                remove_old_files(dir);
                let _ = fs::remove_dir_all(dir.join(PORTABLE_STAGING));
            }
        }
        Kind::MacApp => {
            if let Some(parent) = exe.ancestors().nth(4) {
                for entry in fs::read_dir(parent).into_iter().flatten().flatten() {
                    if entry.file_name().to_string_lossy().starts_with(MAC_STAGING_PREFIX) {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    }
}

fn remove_old_files(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            remove_old_files(&path);
        } else if entry.file_name().to_string_lossy().contains(OLD_SUFFIX) {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nebula-updater-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct TestKey {
        public: String,
        secret: minisign::SecretKey,
    }

    fn test_key() -> TestKey {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        TestKey { public: pair.pk.to_base64(), secret: pair.sk }
    }

    fn sign(key: &TestKey, data: &[u8], comment: &str) -> String {
        minisign::sign(None, &key.secret, io::Cursor::new(data), Some(comment), None).unwrap().into_string()
    }

    #[test]
    fn the_real_key_parses() {
        minisign_verify::PublicKey::from_base64(PUBLIC_KEY).unwrap();
    }

    #[test]
    fn verifies_signatures() {
        let key = test_key();
        let data = b"new version";
        let sig = sign(&key, data, "file:a.exe");
        assert!(verify(&key.public, data, &sig, "a.exe").is_ok());
        // Changed data, another file's signature, another key.
        assert!(verify(&key.public, b"new versioN", &sig, "a.exe").is_err());
        assert!(verify(&key.public, data, &sig, "b.exe").is_err());
        assert!(verify(&test_key().public, data, &sig, "a.exe").is_err());
        assert!(verify(PUBLIC_KEY, data, &sig, "a.exe").is_err());
        assert!(verify(&key.public, data, "not a signature", "a.exe").is_err());
    }

    /// A signature from the minisign command-line tool, which the release
    /// workflow uses (`echo hello`, signed with a throwaway key).
    #[test]
    fn verifies_the_cli_signature_format() {
        let public = "RWTE7o2hDzLgxo+GLI4LLCWiHfAF+mBb7YbMrKjgrMarPBHjgZdLARVn";
        let sig = "untrusted comment: signature from minisign secret key
RUTE7o2hDzLgxgboiPkohRobK6zjfTykfZozWbfx9hzjzfMYY0Y8uudK+wjY5y/Uczc4p9ZAJHOqtDz9ELiYuA2Nrp+W9Im9TgM=
trusted comment: file:NebulaPlayer-9.9.9-windows-x64-setup.exe
7rVP2TL98MjcHozLnWbt2iVwBsCVGW8Xdhd0eaTgG5wbDHD2P38h/RLjqunCDD9hw4knAy33E1ePabZ5kBCQDA==
";
        assert!(verify(public, b"hello\n", sig, "NebulaPlayer-9.9.9-windows-x64-setup.exe").is_ok());
        assert!(verify(public, b"hello!\n", sig, "NebulaPlayer-9.9.9-windows-x64-setup.exe").is_err());
    }

    fn release(names: &[&str]) -> Release {
        Release {
            version: "0.1.7".into(),
            url: String::new(),
            assets: names.iter().map(|n| Asset { name: n.to_string(), url: format!("https://x/{n}") }).collect(),
        }
    }

    #[test]
    fn picks_the_signed_package() {
        let r = release(&[
            "NebulaPlayer-0.1.7-windows-x64-setup.exe",
            "NebulaPlayer-0.1.7-windows-x64-setup.exe.minisig",
            "NebulaPlayer-0.1.7-windows-x64.zip",
            "NebulaPlayer-0.1.7-macos-arm64.app.tar.gz",
            "NebulaPlayer-0.1.7-macos-arm64.app.tar.gz.minisig",
        ]);
        let plan = |kind| Plan::new(kind, &r, PathBuf::from("x"), PathBuf::from("y"));
        let p = plan(Kind::WindowsInstaller).unwrap();
        assert_eq!(p.file.name, "NebulaPlayer-0.1.7-windows-x64-setup.exe");
        assert_eq!(p.signature.name, "NebulaPlayer-0.1.7-windows-x64-setup.exe.minisig");
        assert_eq!(plan(Kind::MacApp).unwrap().file.name, "NebulaPlayer-0.1.7-macos-arm64.app.tar.gz");
        // The zip has no signature: no in-app update for it.
        assert!(plan(Kind::WindowsPortable).is_none());
        // Releases from before signing.
        assert!(Plan::new(Kind::WindowsInstaller, &release(&["NebulaPlayer-0.1.7-windows-x64-setup.exe"]), "x".into(), "y".into()).is_none());
    }

    #[test]
    fn mac_location_check() {
        assert!(check_mac_location(Path::new("/Applications/Nebula Player.app")).is_ok());
        assert!(check_mac_location(Path::new("/Users/t/Applications/Nebula Player.app")).is_ok());
        assert!(check_mac_location(Path::new("/Volumes/Nebula Player/Nebula Player.app")).is_err());
        assert!(check_mac_location(Path::new(
            "/private/var/folders/x/T/AppTranslocation/ABC/d/Nebula Player.app"
        ))
        .is_err());
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            for (name, data) in files {
                zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn unpacks_the_release_zip() {
        let dir = temp_dir("zip");
        let data = zip_of(&[
            ("NebulaPlayer-0.1.7-windows-x64/nebula-player.exe", b"exe"),
            ("NebulaPlayer-0.1.7-windows-x64/archive.dll", b"dll"),
            ("NebulaPlayer-0.1.7-windows-x64/sub/x.txt", b"x"),
        ]);
        extract_zip(&data, &dir).unwrap();
        assert_eq!(fs::read(dir.join("nebula-player.exe")).unwrap(), b"exe");
        assert_eq!(fs::read(dir.join("sub/x.txt")).unwrap(), b"x");
        assert!(extract_zip(&zip_of(&[("../evil.dll", b"x")]), &dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn swaps_files_in_and_cleans_up() {
        let dir = temp_dir("swap");
        let (app, staging) = (dir.join("app"), dir.join("app").join(PORTABLE_STAGING));
        fs::create_dir_all(&staging).unwrap();
        fs::write(app.join("nebula-player.exe"), b"old").unwrap();
        fs::write(app.join("keep.txt"), b"mine").unwrap();
        fs::write(staging.join("nebula-player.exe"), b"new").unwrap();
        fs::write(staging.join("new.dll"), b"dll").unwrap();
        swap_in(&staging, &app).unwrap();
        assert_eq!(fs::read(app.join("nebula-player.exe")).unwrap(), b"new");
        assert_eq!(fs::read(app.join("new.dll")).unwrap(), b"dll");
        assert_eq!(fs::read(app.join("keep.txt")).unwrap(), b"mine");
        assert_eq!(fs::read(app.join("nebula-player.exe.nebula-old")).unwrap(), b"old");
        remove_old_files(&app);
        assert!(!app.join("nebula-player.exe.nebula-old").exists());
        assert!(app.join("keep.txt").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn swap_rolls_back_on_failure() {
        let dir = temp_dir("rollback");
        let (app, staging) = (dir.join("app"), dir.join("staging"));
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(staging.join("b")).unwrap();
        fs::write(app.join("a"), b"old a").unwrap();
        fs::write(staging.join("a"), b"new a").unwrap();
        // A file where the update needs a folder makes the second move fail.
        fs::write(app.join("b"), b"file").unwrap();
        fs::write(staging.join("b").join("c"), b"new c").unwrap();
        assert!(swap_in(&staging, &app).is_err());
        assert_eq!(fs::read(app.join("a")).unwrap(), b"old a");
        assert_eq!(fs::read(app.join("b")).unwrap(), b"file");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The portable update renames the running exe aside; Windows allows
    /// that (but not deleting or overwriting it).
    #[cfg(windows)]
    #[test]
    fn running_exe_can_be_renamed() {
        let exe = std::env::current_exe().unwrap();
        let aside = exe.with_extension("exe.nebula-old");
        fs::rename(&exe, &aside).unwrap();
        fs::rename(&aside, &exe).unwrap();
    }

    fn wait_for(path: &Path, secs: u64) -> bool {
        for _ in 0..secs * 10 {
            if path.exists() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

    /// Runs the installer line the way `apply` does, with a batch file
    /// standing in for the setup and for the app, in folders with spaces.
    #[cfg(windows)]
    #[test]
    fn windows_installer_script_runs() {
        let dir = temp_dir("win script").join("Program Files (x86)");
        fs::create_dir_all(&dir).unwrap();
        let (args, started) = (dir.join("setup args.txt"), dir.join("app started.txt"));
        let setup = dir.join("fake setup.bat");
        let app = dir.join("fake app.bat");
        fs::write(&setup, format!("@echo %* > \"{}\"\r\n", args.display())).unwrap();
        fs::write(&app, format!("@echo ok > \"{}\"\r\n@exit\r\n", started.display())).unwrap();
        let log = dir.join("setup.log");
        run_after_exit_windows(&installer_script(&setup, &log, &app)).unwrap();
        assert!(wait_for(&started, 30), "the app wasn't started");
        let args = fs::read_to_string(&args).unwrap();
        assert!(args.contains("/VERYSILENT") && args.contains("/NORESTARTAPPLICATIONS"), "{args}");
        assert!(args.contains(&format!("\"/LOG={}\"", log.display())), "{args}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_relaunch_script_runs() {
        let dir = temp_dir("win relaunch").join("Nebula Player");
        fs::create_dir_all(&dir).unwrap();
        let started = dir.join("started.txt");
        let app = dir.join("nebula app.bat");
        fs::write(&app, format!("@echo ok > \"{}\"\r\n@exit\r\n", started.display())).unwrap();
        run_after_exit_windows(&relaunch_script(&app)).unwrap();
        assert!(wait_for(&started, 30), "the app wasn't started");
    }

    /// The restart shell waits for the old process, removes the staging
    /// folder and opens the app (a minimal bundle whose program leaves a
    /// file behind).
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_restart_waits_then_opens() {
        let dir = temp_dir("mac restart");
        let app = dir.join("Nebula Test.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        let started = dir.join("started");
        fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>run</string>
<key>CFBundleIdentifier</key><string>io.github.NotTimm.NebulaPlayer.updatetest</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#,
        )
        .unwrap();
        let exe = app.join("Contents/MacOS/run");
        fs::write(&exe, format!("#!/bin/sh\ntouch '{}'\n", started.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let staging = dir.join(".nebula-update-9");
        fs::create_dir_all(&staging).unwrap();

        // Stands in for the old app; reaped as soon as it ends.
        let mut old = std::process::Command::new("sleep").arg("2").spawn().unwrap();
        mac_restart(old.id(), &app, &staging).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        assert!(staging.exists(), "didn't wait for the old process");
        assert!(!started.exists(), "opened the app too early");
        old.wait().unwrap();
        assert!(wait_for(&started, 30), "the app wasn't opened");
        assert!(!staging.exists(), "the staging folder was left behind");
    }

    #[test]
    fn swaps_the_mac_bundle() {
        let dir = temp_dir("bundle");
        let (app, new, old) = (dir.join("Nebula Player.app"), dir.join("s/Nebula Player.app"), dir.join("s/previous.app"));
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(new.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/nebula-player"), b"old").unwrap();
        fs::write(new.join("Contents/MacOS/nebula-player"), b"new").unwrap();
        swap_bundle(&new, &app, &old).unwrap();
        assert_eq!(fs::read(app.join("Contents/MacOS/nebula-player")).unwrap(), b"new");
        assert_eq!(fs::read(old.join("Contents/MacOS/nebula-player")).unwrap(), b"old");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Serves `files` over HTTP on localhost, one connection per request.
    fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("").trim_start_matches('/').to_string();
                match files.iter().find(|(name, _)| *name == path) {
                    Some((_, body)) => {
                        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            }
        });
        format!("http://{addr}")
    }

    fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    /// The whole portable flow against a local server: download, signature
    /// check, unpack, swap.
    #[test]
    fn updates_a_portable_folder_end_to_end() {
        let key = test_key();
        let name = "NebulaPlayer-0.1.7-windows-x64.zip";
        let zip = zip_of(&[
            ("NebulaPlayer-0.1.7-windows-x64/nebula-player.exe", b"new exe"),
            ("NebulaPlayer-0.1.7-windows-x64/archive.dll", b"new dll"),
        ]);
        let good = sign(&key, &zip, &format!("file:{name}"));
        let bad = sign(&test_key(), &zip, &format!("file:{name}"));
        let base = serve(vec![
            (name.to_string(), zip.clone()),
            (format!("{name}.minisig"), good.into_bytes()),
            ("bad.minisig".to_string(), bad.into_bytes()),
        ]);
        let dir = temp_dir("e2e");
        let app = dir.join("Nebula");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("nebula-player.exe"), b"old exe").unwrap();
        let plan = |sig: &str| Plan {
            kind: Kind::WindowsPortable,
            version: "0.1.7".into(),
            file: Asset { name: name.into(), url: format!("{base}/{name}") },
            signature: Asset { name: format!("{name}.minisig"), url: format!("{base}/{sig}") },
            exe: app.join("nebula-player.exe"),
            app: app.clone(),
        };

        // Signed with another key: refused before anything is unpacked.
        let err = run(fetch_and_stage(plan("bad.minisig"), &key.public, |_, _| {})).unwrap_err();
        assert!(err.contains("signed"), "{err}");
        assert!(!app.join(PORTABLE_STAGING).exists());

        let mut seen = 0;
        let staged = run(fetch_and_stage(plan(&format!("{name}.minisig")), &key.public, |done, total| {
            assert_eq!(total, Some(zip.len() as u64));
            seen = done;
        }))
        .unwrap();
        assert_eq!(seen, zip.len() as u64);
        assert_eq!(fs::read(app.join("nebula-player.exe")).unwrap(), b"old exe");
        swap_in(&staged.path, &app).unwrap();
        assert_eq!(fs::read(app.join("nebula-player.exe")).unwrap(), b"new exe");
        assert_eq!(fs::read(app.join("archive.dll")).unwrap(), b"new dll");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The Mac flow with a real tar.gz of a bundle (tar is on every
    /// platform CI runs on).
    #[test]
    fn updates_a_mac_bundle_end_to_end() {
        let key = test_key();
        let dir = temp_dir("mac-e2e");
        let build = dir.join("build");
        fs::create_dir_all(build.join("Nebula Player.app/Contents/MacOS")).unwrap();
        fs::write(build.join("Nebula Player.app/Contents/MacOS/nebula-player"), b"new").unwrap();
        let archive = dir.join("a.tar.gz");
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&build)
            .arg("Nebula Player.app")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let data = fs::read(&archive).unwrap();
        let name = "NebulaPlayer-0.1.7-macos-arm64.app.tar.gz";
        let base = serve(vec![
            (name.to_string(), data.clone()),
            (format!("{name}.minisig"), sign(&key, &data, &format!("file:{name}")).into_bytes()),
        ]);
        let apps = dir.join("Applications");
        let app = apps.join("Nebula Player.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/nebula-player"), b"old").unwrap();
        let plan = Plan {
            kind: Kind::MacApp,
            version: "0.1.7".into(),
            file: Asset { name: name.into(), url: format!("{base}/{name}") },
            signature: Asset { name: format!("{name}.minisig"), url: format!("{base}/{name}.minisig") },
            exe: app.join("Contents/MacOS/nebula-player"),
            app: app.clone(),
        };
        let staged = run(fetch_and_stage(plan, &key.public, |_, _| {})).unwrap();
        assert!(staged.path.starts_with(&apps));
        swap_bundle(&staged.path, &app, &staged.path.parent().unwrap().join("previous.app")).unwrap();
        assert_eq!(fs::read(app.join("Contents/MacOS/nebula-player")).unwrap(), b"new");
        fs::remove_dir_all(&dir).unwrap();
    }
}
