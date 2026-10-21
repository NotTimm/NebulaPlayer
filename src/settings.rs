//! Preferences, saved as `settings.json` in the config folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Jellyfin's dark look and blue-purple accent.
    #[default]
    Jellyfin,
    /// The desktop's own colours (COSMIC's theme, light or dark).
    System,
}

/// The accent colour of the Jellyfin theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccentChoice {
    /// The desktop's accent, when it has one; Jellyfin's blue otherwise.
    #[default]
    Desktop,
    /// Always Jellyfin's blue.
    Jellyfin,
}

impl AccentChoice {
    pub const ALL: [AccentChoice; 2] = [AccentChoice::Desktop, AccentChoice::Jellyfin];
    pub const LABELS: [&'static str; 2] = ["Desktop's accent", "Jellyfin blue"];
}

/// What happens at an intro, credits, recap or preview the server knows
/// about (Jellyfin's media segments, from Intro Skipper and the like).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipMode {
    /// mpv shows "Skip Intro: press S or Enter".
    #[default]
    Ask,
    Auto,
    Off,
}

impl SkipMode {
    pub const ALL: [SkipMode; 3] = [SkipMode::Ask, SkipMode::Auto, SkipMode::Off];
    pub const LABELS: [&'static str; 3] = ["Ask to skip", "Skip automatically", "Don't skip"];
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub accent: AccentChoice,
    /// The signed-in account shown (`Account::key`).
    pub account: Option<String>,
    /// Video starts full screen.
    pub fullscreen: bool,
    /// Play in mpv's own window (with its own controls and keys) rather
    /// than inside the app's.
    pub separate_window: bool,
    /// Hardware decoding (mpv's `hwdec=auto-safe`).
    pub hwdec: bool,
    /// Episodes play on into the next one.
    pub autoplay_next: bool,
    /// Intros, credits and the like.
    pub skip: SkipMode,
    /// Folders listed on the Local Files page.
    pub local_folders: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Jellyfin,
            accent: AccentChoice::Desktop,
            account: None,
            fullscreen: false,
            separate_window: false,
            hwdec: true,
            autoplay_next: true,
            skip: SkipMode::Ask,
            local_folders: Vec::new(),
        }
    }
}

fn path() -> PathBuf {
    crate::platform::config_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_vec_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("couldn't save settings: {e}");
                }
            }
            Err(e) => log::warn!("couldn't save settings: {e}"),
        }
    }
}
