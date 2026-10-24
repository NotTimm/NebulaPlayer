//! What version this is, how it was installed, and whether GitHub has a
//! newer release.
//!
//! The check is one unauthenticated request to GitHub's releases API. The
//! Windows installer, the Windows zip and the Mac app can update themselves
//! (`crate::updater`); every other install updates through its own channel
//! (COPR, a .flatpak file, rpm/deb, AppImage, or built from source), so
//! the About section says what to do for that install.

use std::sync::OnceLock;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/NotTimm/NebulaPlayer";
pub const RELEASES_URL: &str = "https://github.com/NotTimm/NebulaPlayer/releases";
/// Every release, newest first: they're all pre-releases (alpha, beta),
/// which `/releases/latest` leaves out.
const RELEASES_API: &str = "https://api.github.com/repos/NotTimm/NebulaPlayer/releases?per_page=10";

/// The commit this was built from (set by build.rs), if it was built from a
/// git checkout.
pub fn build_commit() -> Option<&'static str> {
    option_env!("NEBULA_COMMIT").filter(|c| !c.is_empty())
}

/// Set by a distribution's package build, for example
/// `NEBULA_DISTRO="Fedora package"`: that distro's repositories bring new
/// versions, so the app doesn't ask GitHub, and About shows this label.
pub fn distro() -> Option<&'static str> {
    option_env!("NEBULA_DISTRO").filter(|d| !d.is_empty())
}

/// How this copy was installed, as far as we can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    Flatpak,
    AppImage,
    Rpm,
    Deb,
    /// Built by a distribution for its repositories (`distro`).
    Distro,
    /// Under ~/.local, from install.sh.
    Source,
    /// Per-user install from the setup .exe.
    WindowsInstaller,
    WindowsPortable,
    MacApp,
    Unknown,
}

impl Install {
    pub fn detect() -> Self {
        if distro().is_some() {
            return Install::Distro;
        }
        if std::env::var_os("FLATPAK_ID").is_some() || std::path::Path::new("/.flatpak-info").exists() {
            return Install::Flatpak;
        }
        if std::env::var_os("APPIMAGE").is_some() {
            return Install::AppImage;
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if cfg!(windows) {
            // Inno Setup leaves its uninstaller next to the exe, for
            // per-user and all-users installs alike.
            let installed = std::env::current_exe()
                .ok()
                .and_then(|p| Some(p.parent()?.join("unins000.exe").exists()))
                .unwrap_or(false);
            return if installed || exe.to_ascii_lowercase().contains("\\appdata\\local\\programs\\") {
                Install::WindowsInstaller
            } else {
                Install::WindowsPortable
            };
        }
        if cfg!(target_os = "macos") {
            return if exe.contains(".app/Contents/MacOS/") { Install::MacApp } else { Install::Unknown };
        }
        if exe.starts_with("/usr/") {
            if std::path::Path::new("/etc/debian_version").exists() {
                return Install::Deb;
            }
            if std::path::Path::new("/usr/bin/rpm").exists() {
                return Install::Rpm;
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            if exe.starts_with(&*std::path::Path::new(&home).join(".local").to_string_lossy()) {
                return Install::Source;
            }
        }
        Install::Unknown
    }

    /// One line on how to get a newer version for this install.
    pub fn how_to_update(self) -> &'static str {
        match self {
            Install::Flatpak => {
                "Download the new .flatpak from the release page and install it with \
                 `flatpak install --user NebulaPlayer-*.flatpak`."
            }
            Install::AppImage => "Download the new AppImage from the release page and replace this one.",
            Install::Rpm => "Download the new .rpm from the release page and install it with `sudo dnf install ./nebula-player-*.rpm`.",
            Install::Deb => "Download the new .deb from the release page and install it with `sudo apt install ./nebula-player_*.deb`.",
            Install::Distro => "Your system's package manager brings in new versions (`sudo dnf upgrade` on Fedora).",
            Install::Source => "Pull the latest code in your checkout and build it again.",
            Install::WindowsInstaller => "Download and run the new installer from the release page.",
            Install::WindowsPortable => "Download the new .zip from the release page and replace this folder.",
            Install::MacApp => "Download the new .dmg from the release page and drag Nebula Player into Applications again.",
            Install::Unknown => "Get the new version from the release page.",
        }
    }

    /// Whether this install looks for new releases on GitHub. A distro's
    /// package waits for its repositories, which may lag behind.
    pub fn checks_github(self) -> bool {
        self != Install::Distro
    }

    pub fn label(self) -> &'static str {
        match self {
            Install::Flatpak => "Flatpak",
            Install::AppImage => "AppImage",
            Install::Rpm => "RPM package",
            Install::Deb => "Debian package",
            Install::Distro => distro().unwrap_or("Distribution package"),
            Install::Source => "Built from source",
            Install::WindowsInstaller => "Windows installer",
            Install::WindowsPortable => "Windows (portable)",
            Install::MacApp => "macOS app",
            Install::Unknown => "Unknown",
        }
    }
}

/// The newest release on GitHub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// The files attached to the release.
    pub assets: Vec<Asset>,
}

/// A file attached to a release.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
}

#[derive(serde::Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            // GitHub's API refuses requests without a User-Agent.
            .user_agent(concat!("nebula-player/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client")
    })
}

/// The newest release, pre-releases included, or `None` when this is
/// already the newest.
pub async fn check() -> Result<Option<Release>, String> {
    let resp = client()
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("couldn't reach GitHub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub answered HTTP {}", resp.status()));
    }
    let releases: Vec<GhRelease> = resp.json().await.map_err(|e| e.to_string())?;
    Ok(newest(releases, VERSION))
}

/// The newest of `releases` (by version, not by date), if it's newer than
/// `current`.
fn newest(releases: Vec<GhRelease>, current: &str) -> Option<Release> {
    releases
        .into_iter()
        .filter(|r| !r.draft)
        .map(|r| Release { version: r.tag_name.trim_start_matches('v').to_string(), url: r.html_url, assets: r.assets })
        .fold(None, |best: Option<Release>, r| match best {
            Some(b) if !is_newer(&r.version, &b.version) => Some(b),
            _ => Some(r),
        })
        .filter(|r| is_newer(&r.version, current))
}

/// Whether `candidate` is a later version than `current` (semver-style:
/// `0.1.10` > `0.1.9`, and `0.2.0` > `0.2.0-beta.3`).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<([u64; 3], Option<&str>)> {
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (v, None),
        };
        let mut nums = [0u64; 3];
        for (i, part) in core.split('.').enumerate() {
            *nums.get_mut(i)? = part.parse().ok()?;
        }
        Some((nums, pre))
    }
    let (Some((a, a_pre)), Some((b, b_pre))) = (parse(candidate), parse(current)) else { return false };
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (a_pre, b_pre) {
            (None, Some(_)) => true,
            (Some(x), Some(y)) => pre_newer(x, y),
            _ => false,
        },
    }
}

/// Compares pre-release tags like `beta.3` and `beta.10` part by part.
fn pre_newer(a: &str, b: &str) -> bool {
    for (x, y) in a.split('.').zip(b.split('.')) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord == std::cmp::Ordering::Greater;
        }
    }
    a.split('.').count() > b.split('.').count()
}

#[cfg(test)]
mod tests {
    use super::{is_newer, newest, GhRelease};

    fn gh(tag: &str, draft: bool) -> GhRelease {
        GhRelease { tag_name: tag.into(), html_url: format!("https://x/{tag}"), draft, assets: Vec::new() }
    }

    #[test]
    fn finds_the_newest_pre_release() {
        let list = || vec![gh("v0.1.0-alpha.5", true), gh("v0.1.0-alpha.4", false), gh("v0.1.0-alpha.10", false), gh("v0.1.0-alpha.2", false)];
        assert_eq!(newest(list(), "0.1.0-alpha.4").unwrap().version, "0.1.0-alpha.10");
        assert!(newest(list(), "0.1.0-alpha.10").is_none());
        // Drafts don't count.
        assert!(newest(vec![gh("v0.1.0-alpha.5", true)], "0.1.0-alpha.4").is_none());
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.1.3", "0.1.2"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("0.2.0", "0.1.12"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(!is_newer("0.1.1", "0.1.2"));
        assert!(is_newer("0.2.0", "0.2.0-beta.3"));
        assert!(!is_newer("0.2.0-beta.3", "0.2.0"));
        assert!(is_newer("0.1.0-beta.10", "0.1.0-beta.9"));
        assert!(is_newer("0.1.0-alpha.10", "0.1.0-alpha.9"));
        assert!(is_newer("0.1.0-beta.1", "0.1.0-alpha.9"));
        assert!(!is_newer("garbage", "0.1.2"));
    }
}
