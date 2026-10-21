// No console window behind the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod images;
mod jellyfin;
mod mpv;
mod platform;
mod settings;
mod theme;

use cosmic::app::Settings;
use cosmic::iced::Size;

pub const APP_ID: &str = "io.github.NotTimm.NebulaPlayer";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // A video file given on the command line (or by "Open With") plays.
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    // Posters and backdrops are cache: keep them under 1 GB.
    images::prune(1024 * 1024 * 1024);

    let saved = settings::Settings::load();
    let mut settings = Settings::default()
        .size(Size::new(1280.0, 820.0))
        .theme(theme::build(saved.theme))
        // Windows and macOS draw their own title bar.
        .client_decorations(cfg!(target_os = "linux"));
    if let Some(icons) = platform::fallback_icon_theme() {
        settings = settings.default_icon_theme(icons);
    }

    cosmic::app::run::<app::App>(settings, path)?;
    Ok(())
}
