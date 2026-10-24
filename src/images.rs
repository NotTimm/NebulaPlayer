//! Posters, thumbs and backdrops: fetched at the size they're shown,
//! kept on disk (the server's image tag is in the file name, so a changed
//! picture is a new file), and decoded off the UI thread.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;

use cosmic::widget::image::Handle;
use tokio::sync::{mpsc, Mutex, Semaphore};

use crate::jellyfin::{Account, ImageRef, TrickplayInfo};

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

/// A picture that finished loading, or why it couldn't.
pub type Arrival = (Key, Result<Picture, String>);

/// Waits this long after a picture arrives for others to join it: the page
/// rebuilds once per batch, not once per picture (a library scrolled into
/// new rows brings dozens).
const BATCH_WINDOW: std::time::Duration = std::time::Duration::from_millis(40);

fn arrivals_queue() -> &'static (mpsc::UnboundedSender<Arrival>, Mutex<mpsc::UnboundedReceiver<Arrival>>) {
    static Q: OnceLock<(mpsc::UnboundedSender<Arrival>, Mutex<mpsc::UnboundedReceiver<Arrival>>)> = OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        (tx, Mutex::new(rx))
    })
}

/// Loads `key` and hands it to `arrivals`.
pub async fn load_to_queue(account: Account, key: Key) {
    let result = load(account, key.clone()).await;
    let _ = arrivals_queue().0.send((key, result));
}

/// Loaded pictures, in batches (a subscription).
pub fn arrivals() -> impl cosmic::iced::futures::Stream<Item = Vec<Arrival>> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::stream::channel(4, async |mut out: cosmic::iced::futures::channel::mpsc::Sender<Vec<Arrival>>| {
        let mut rx = arrivals_queue().1.lock().await;
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            tokio::time::sleep(BATCH_WINDOW).await;
            while let Ok(more) = rx.try_recv() {
                batch.push(more);
            }
            if out.send(batch).await.is_err() {
                break;
            }
        }
    })
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

/// One sheet of an item's trickplay thumbnails.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SheetKey {
    pub item_id: String,
    pub source_id: String,
    /// The manifest is part of the key, so thumbnails the server made
    /// again differently are fetched again.
    pub info: TrickplayInfo,
    pub index: u32,
}

/// A trickplay sheet, cut into its thumbnails: from the disk cache, or
/// downloaded once (sharing the download limit with the posters).
pub async fn load_sheet(account: Account, key: SheetKey) -> Result<Vec<Handle>, String> {
    let path = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ("trickplay", &account.server_id, &key).hash(&mut h);
        dir().join(format!("{:016x}", h.finish()))
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => {
            let _slot = slots().acquire().await.map_err(|e| e.to_string())?;
            let b = crate::jellyfin::trickplay_sheet(&account, &key.item_id, &key.source_id, key.info.width, key.index).await?;
            let _ = tokio::fs::create_dir_all(dir()).await;
            let _ = tokio::fs::write(&path, &b).await;
            b
        }
    };
    tokio::task::spawn_blocking(move || cut_sheet(&bytes, &key.info, key.index))
        .await
        .map_err(|e| e.to_string())?
}

/// Cuts a decoded sheet into its thumbnails, across then down.
fn cut_sheet(bytes: &[u8], info: &TrickplayInfo, index: u32) -> Result<Vec<Handle>, String> {
    let sheet = image::load_from_memory(bytes).map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = (info.width, info.height);
    let tiles = (0..info.tiles_on(index))
        .map_while(|t| {
            let (x, y) = ((t % info.tile_width) * w, (t / info.tile_width) * h);
            (x + w <= sheet.width() && y + h <= sheet.height()).then(|| {
                let tile = image::imageops::crop_imm(&sheet, x, y, w, h).to_image();
                Handle::from_rgba(w, h, tile.into_raw())
            })
        })
        .collect::<Vec<_>>();
    if tiles.is_empty() {
        return Err(format!("trickplay sheet {index} is smaller than its thumbnails"));
    }
    Ok(tiles)
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
