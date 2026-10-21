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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// The signed-in account shown (`Account::key`).
    pub account: Option<String>,
    /// mpv opens full screen.
    pub fullscreen: bool,
    /// Hardware decoding (mpv's `hwdec=auto-safe`).
    pub hwdec: bool,
    /// Episodes play on into the next one.
    pub autoplay_next: bool,
    /// Folders listed on the Local Files page.
    pub local_folders: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Jellyfin,
            account: None,
            fullscreen: false,
            hwdec: true,
            autoplay_next: true,
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
