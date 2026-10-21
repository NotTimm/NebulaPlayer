//! Local files: folders the user adds, listed by file name. No scraping;
//! mpv remembers where each file was stopped.

use std::path::{Path, PathBuf};

use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, icon};
use cosmic::Element;

use super::{App, Message};

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "webm", "mov", "wmv", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "ogv", "3gp",
];

/// Folders this deep below an added one are still looked in.
const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug)]
pub struct LocalFile {
    pub path: PathBuf,
    /// The path below the added folder, for display.
    pub name: String,
    pub size: u64,
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Lists the videos under each folder, sorted by path.
pub fn scan(folders: &[PathBuf]) -> Vec<(PathBuf, Vec<LocalFile>)> {
    folders
        .iter()
        .map(|root| {
            let mut files = Vec::new();
            walk(root, root, 0, &mut files);
            files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            (root.clone(), files)
        })
        .collect()
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<LocalFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Hidden files, macOS leftovers and Windows' recycle bin.
        if name.starts_with('.') || name.eq_ignore_ascii_case("$RECYCLE.BIN") {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, depth + 1, out);
            }
        } else if is_video(&path) {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let name = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
            out.push(LocalFile { path, name, size });
        }
    }
}

fn human_size(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

impl App {
    pub(super) fn view_local(&self) -> Element<'_, Message> {
        let header = widget::row::with_capacity(3)
            .push(widget::text::title2("Local Files"))
            .push(widget::Space::new().width(Length::Fill).height(Length::Shrink))
            .push(widget::button::standard("Add Folder").leading_icon(icon::from_name("list-add-symbolic")).on_press(Message::AddFolder))
            .push(widget::button::standard("Play a File").on_press(Message::OpenFile))
            .spacing(12)
            .align_y(Alignment::Center);
        let mut col = widget::column::with_capacity(4).spacing(20).padding([20, 28, 28, 28]).push(header);
        if self.settings.local_folders.is_empty() {
            col = col.push(widget::text::body(
                "Add a folder to list its videos here. They play in mpv, which remembers where you stopped. \
                 Titles, posters and the rest come from Jellyfin only.",
            ));
        }
        if self.local_scanning && self.local.is_empty() {
            col = col.push(widget::text::caption("Looking for videos…"));
        }
        for (i, (root, files)) in self.local.iter().enumerate() {
            let mut section = widget::settings::section().title(root.to_string_lossy().into_owned());
            if files.is_empty() {
                section = section.add(widget::settings::item_row(vec![widget::text::body("No videos found.").into()]));
            }
            for f in files {
                let row = widget::row::with_capacity(3)
                    .push(icon::from_name("video-x-generic-symbolic").size(16))
                    .push(widget::text::body(f.name.clone()).width(Length::Fill))
                    .push(widget::text::caption(human_size(f.size)))
                    .spacing(12)
                    .align_y(Alignment::Center);
                section = section.add(
                    widget::button::custom(row)
                        .class(cosmic::theme::Button::ListItem([4.0; 4]))
                        .padding([6, 8])
                        .width(Length::Fill)
                        .on_press(Message::PlayLocal(f.path.clone())),
                );
            }
            col = col.push(section);
            col = col.push(
                widget::button::text("Remove this folder")
                    .leading_icon(icon::from_name("list-remove-symbolic"))
                    .on_press(Message::RemoveFolder(i)),
            );
        }
        super::widgets::page_scroll(col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_videos_and_skips_hidden() {
        let dir = std::env::temp_dir().join(format!("nebula-player-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Show/Season 1")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("Show/Season 1/e01.MKV"), b"x").unwrap();
        std::fs::write(dir.join("movie.mp4"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join(".hidden/secret.mkv"), b"x").unwrap();
        let found = scan(std::slice::from_ref(&dir));
        let names: Vec<_> = found[0].1.iter().map(|f| f.name.replace('\\', "/")).collect();
        assert_eq!(names, vec!["movie.mp4", "Show/Season 1/e01.MKV"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
