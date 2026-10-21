//! Posters, thumbs and backdrops: fetched at the size they're shown,
//! kept on disk (the server's image tag is in the file name, so a changed
//! picture is a new file), and decoded off the UI thread.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;

use cosmic::widget::image::Handle;
use tokio::sync::Semaphore;

use crate::jellyfin::{Account, ImageRef};

/// An image at one requested width.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub img: ImageRef,
    pub width: u32,
}

fn dir() -> PathBuf {
    crate::platform::cache_dir().join("images")
}

fn file(account: &Account, key: &Key) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    account.server_id.hash(&mut h);
    key.hash(&mut h);
    dir().join(format!("{:016x}", h.finish()))
}

/// At most this many images download at once, so a big library page
/// doesn't flood the server.
fn slots() -> &'static Semaphore {
    static S: OnceLock<Semaphore> = OnceLock::new();
    S.get_or_init(|| Semaphore::new(8))
}

/// A decoded image and its size in pixels.
#[derive(Clone, Debug)]
pub struct Picture {
    pub handle: Handle,
    pub width: u32,
    pub height: u32,
}

pub async fn load(account: Account, key: Key) -> Result<Picture, String> {
    let path = file(&account, &key);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => {
            let _slot = slots().acquire().await.map_err(|e| e.to_string())?;
            let b = crate::jellyfin::image(&account, &key.img, key.width).await?;
            let _ = tokio::fs::create_dir_all(dir()).await;
            let _ = tokio::fs::write(&path, &b).await;
            b
        }
    };
    tokio::task::spawn_blocking(move || decode(&bytes))
        .await
        .map_err(|e| e.to_string())?
}

/// Decodes to RGBA, so drawing it never stalls the UI.
pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?.into_rgba8();
    let (width, height) = img.dimensions();
    Ok(Picture { handle: Handle::from_rgba(width, height, img.into_raw()), width, height })
}

/// Keeps the image cache under `limit` bytes, dropping the least recently
/// written first.
pub fn prune(limit: u64) {
    let Ok(entries) = std::fs::read_dir(dir()) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= limit {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}
