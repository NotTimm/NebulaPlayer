//! Where the app keeps its files on each OS, and small OS helpers.

use std::path::PathBuf;

/// The user's home directory.
pub fn home_dir() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "linux")]
fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    let root = std::env::var(var)
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(fallback));
    app_dir(root)
}

#[cfg(windows)]
fn known_dir(var: &str, fallback: &str) -> PathBuf {
    let root = std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home_dir().join(fallback));
    app_dir(root)
}

/// `nebula-player` on Linux, `NebulaPlayer` elsewhere.
fn app_dir(root: PathBuf) -> PathBuf {
    #[cfg(target_os = "linux")]
    return root.join("nebula-player");
    #[cfg(not(target_os = "linux"))]
    return root.join("NebulaPlayer");
}

/// Durable data: mpv's resume points for local files.
/// `~/.local/share/nebula-player`, `%APPDATA%\NebulaPlayer`, or
/// `~/Library/Application Support/NebulaPlayer`.
pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    return xdg_dir("XDG_DATA_HOME", ".local/share");
    #[cfg(windows)]
    return known_dir("APPDATA", r"AppData\Roaming");
    #[cfg(target_os = "macos")]
    return app_dir(home_dir().join("Library/Application Support"));
}

/// User preferences (and the app's own mpv.conf). On Windows and macOS
/// these sit next to the data.
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    return xdg_dir("XDG_CONFIG_HOME", ".config");
    #[cfg(not(target_os = "linux"))]
    return data_dir();
}

/// Posters and backdrops, safe to delete. `~/.cache/nebula-player`,
/// `%LOCALAPPDATA%\NebulaPlayer\Cache`, or `~/Library/Caches/NebulaPlayer`.
pub fn cache_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    return xdg_dir("XDG_CACHE_HOME", ".cache");
    #[cfg(windows)]
    return known_dir("LOCALAPPDATA", r"AppData\Local").join("Cache");
    #[cfg(target_os = "macos")]
    return app_dir(home_dir().join("Library/Caches"));
}

/// mpv's settings for this app: `mpv.conf`, `input.conf`, `scripts/`.
pub fn mpv_config_dir() -> PathBuf {
    config_dir().join("mpv")
}

/// Opens a folder or web URL with the system's default handler.
pub fn open(target: &str) {
    #[cfg(windows)]
    let result = {
        use std::os::windows::process::CommandExt;
        // `start` is a cmd built-in; the empty "" is its window title.
        // CREATE_NO_WINDOW stops a console from flashing up.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .creation_flags(0x0800_0000)
            .spawn()
    };
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();

    if let Err(e) = result {
        log::warn!("could not open {target}: {e}");
    }
}

/// libcosmic looks icons up in COSMIC's theme only. Outside COSMIC that
/// theme is often not installed, which would leave every icon blank, so the
/// first of the usual themes that is installed is used instead.
#[cfg(target_os = "linux")]
pub fn fallback_icon_theme() -> Option<&'static str> {
    let mut dirs: Vec<PathBuf> = vec![home_dir().join(".local/share/icons"), home_dir().join(".icons")];
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(data.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    let installed = |name: &str| dirs.iter().any(|d| d.join(name).join("index.theme").exists());
    if installed("Cosmic") {
        return None;
    }
    let theme = ["breeze", "Adwaita", "Papirus", "elementary"].into_iter().find(|t| installed(t))?;
    log::info!("COSMIC icons not installed; using {theme}");
    Some(theme)
}

/// Windows and macOS use the icons bundled into libcosmic.
#[cfg(not(target_os = "linux"))]
pub fn fallback_icon_theme() -> Option<&'static str> {
    None
}
