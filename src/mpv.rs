//! A small binding to libmpv's client API, and the player built on it.
//!
//! mpv opens and draws its own window (with its own on-screen controller),
//! decodes, and handles every key and mouse binding a user knows from mpv.
//! The app tells it what to play and reads back where it is: a background
//! thread waits on mpv's events and keeps a [`State`] the UI polls.
//!
//! Only the handful of client API calls the app needs are bound here, all
//! present since libmpv 1.x, so any distro's libmpv works.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Mutex};

// ── FFI ─────────────────────────────────────────────────────────────────────

#[repr(C)]
struct MpvHandle {
    _private: [u8; 0],
}

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventEndFile {
    reason: c_int,
    error: c_int,
}

const FORMAT_NONE: c_int = 0;
const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;

const EVENT_SHUTDOWN: c_int = 1;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_PROPERTY_CHANGE: c_int = 22;

const END_EOF: c_int = 0;
const END_STOP: c_int = 2;
const END_QUIT: c_int = 3;
const END_ERROR: c_int = 4;

// libmpv.so on Linux; on Windows mpv.lib, the import library CI makes
// for libmpv-2.dll (see build.rs).
#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut MpvHandle;
    fn mpv_initialize(ctx: *mut MpvHandle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut MpvHandle);
    fn mpv_set_option_string(ctx: *mut MpvHandle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_set_property_string(ctx: *mut MpvHandle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_command(ctx: *mut MpvHandle, args: *const *const c_char) -> c_int;
    fn mpv_observe_property(ctx: *mut MpvHandle, userdata: u64, name: *const c_char, format: c_int) -> c_int;
    fn mpv_wait_event(ctx: *mut MpvHandle, timeout: f64) -> *mut MpvEvent;
    fn mpv_error_string(error: c_int) -> *const c_char;
    fn mpv_client_api_version() -> std::ffi::c_ulong;
}

fn error_string(code: c_int) -> String {
    // SAFETY: mpv returns a static string for every error code.
    unsafe { CStr::from_ptr(mpv_error_string(code)) }.to_string_lossy().into_owned()
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// libmpv's client API version as "major.minor", for the About page.
pub fn api_version() -> String {
    // SAFETY: a plain getter with no arguments.
    let v = unsafe { mpv_client_api_version() } as u64;
    format!("{}.{}", v >> 16, v & 0xffff)
}

/// The mpv handle. mpv's client API is thread-safe, so it's shared between
/// the UI and the event thread; it's destroyed when the last one lets go.
struct Handle(*mut MpvHandle);

// SAFETY: every mpv client API function may be called from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the pointer came from mpv_create and is dropped once.
        unsafe { mpv_terminate_destroy(self.0) };
    }
}

impl Handle {
    fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        // SAFETY: valid handle and NUL-terminated strings for the call.
        let r = unsafe { mpv_set_option_string(self.0, n.as_ptr(), v.as_ptr()) };
        if r < 0 {
            return Err(format!("mpv option {name}={value}: {}", error_string(r)));
        }
        Ok(())
    }

    fn set_property(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        // SAFETY: as above.
        let r = unsafe { mpv_set_property_string(self.0, n.as_ptr(), v.as_ptr()) };
        if r < 0 {
            return Err(format!("mpv property {name}={value}: {}", error_string(r)));
        }
        Ok(())
    }

    fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: a NULL-terminated array of valid C strings that outlive the call.
        let r = unsafe { mpv_command(self.0, ptrs.as_ptr()) };
        if r < 0 {
            return Err(format!("mpv {}: {}", args.first().unwrap_or(&""), error_string(r)));
        }
        Ok(())
    }

    fn observe(&self, id: u64, name: &str, format: c_int) {
        let n = cstr(name);
        // SAFETY: valid handle and name.
        unsafe { mpv_observe_property(self.0, id, n.as_ptr(), format) };
    }
}

// ── Player ──────────────────────────────────────────────────────────────────

/// Why playback of a file ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Played to the end.
    Eof,
    /// Stopped: another file was loaded, or `stop`.
    Stopped,
    /// The file couldn't be played.
    Error,
}

/// What the player is doing, kept up to date by the event thread.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// Seconds into the current file.
    pub position: f64,
    /// The file's length in seconds, 0 until known.
    pub duration: f64,
    pub paused: bool,
    /// Counts files loaded, so the app sees each new one start.
    pub loaded: u64,
    /// Set when a file stops playing; cleared when the next one loads.
    pub ended: Option<End>,
    /// mpv's window was closed (or `quit`): the player is gone.
    pub closed: bool,
    /// The last error mpv reported for a file.
    pub error: Option<String>,
}

/// An external subtitle file to add once its video has loaded.
#[derive(Clone, Debug)]
pub struct ExternalSub {
    pub url: String,
    pub title: String,
    pub lang: String,
    pub select: bool,
}

/// What to play and how.
#[derive(Clone, Debug, Default)]
pub struct Load {
    pub url: String,
    pub title: String,
    /// Where to start, in seconds; `None` lets mpv's own resume apply.
    pub start: Option<f64>,
    /// HTTP headers sent with every request for this file (and its subs).
    pub headers: Vec<String>,
    /// mpv's audio track id (`aid`) to pick, 1-based.
    pub audio: Option<i64>,
    /// mpv's subtitle track id (`sid`); `Some(0)` turns subtitles off.
    pub subtitle: Option<i64>,
    pub external_subs: Vec<ExternalSub>,
    /// mpv's own "remember where I stopped" (`save-position-on-quit`),
    /// for local files; Jellyfin keeps progress on the server instead.
    pub remember_position: bool,
}

/// Options for the mpv instance.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub fullscreen: bool,
    pub hwdec: bool,
    /// Holds `mpv.conf`, `input.conf` and `scripts/` for this app's mpv.
    pub config_dir: std::path::PathBuf,
    pub watch_later_dir: std::path::PathBuf,
}

struct Shared {
    handle: Handle,
    state: Mutex<State>,
    /// Subtitles for the file being loaded, added on FILE_LOADED.
    pending_subs: Mutex<Vec<ExternalSub>>,
}

/// One mpv instance and its window. Dropping it closes the window.
pub struct Player {
    shared: Arc<Shared>,
}

const OBS_TIME: u64 = 1;
const OBS_DURATION: u64 = 2;
const OBS_PAUSE: u64 = 3;

impl Player {
    pub fn new(opts: &Options) -> Result<Self, String> {
        // SAFETY: plain constructor; NULL means out of memory or a bad locale.
        let raw = unsafe { mpv_create() };
        if raw.is_null() {
            return Err("couldn't start mpv".into());
        }
        let handle = Handle(raw);

        let _ = std::fs::create_dir_all(&opts.config_dir);
        let _ = std::fs::create_dir_all(&opts.watch_later_dir);
        // The user's own mpv.conf for this app is read from config_dir;
        // these are the defaults it can override.
        let defaults: &[(&str, &str)] = &[
            ("config-dir", &opts.config_dir.to_string_lossy()),
            ("config", "yes"),
            ("terminal", "no"),
            ("input-default-bindings", "yes"),
            ("input-vo-keyboard", "yes"),
            ("osc", "yes"),
            ("force-window", "yes"),
            // Stay open between files, so the next episode plays in the
            // same window; the app quits it when nothing follows.
            ("idle", "yes"),
            ("keep-open", "no"),
            ("ytdl", "no"),
            ("hwdec", if opts.hwdec { "auto-safe" } else { "no" }),
            ("fullscreen", if opts.fullscreen { "yes" } else { "no" }),
            ("title", "${media-title} — Nebula"),
            ("watch-later-directory", &opts.watch_later_dir.to_string_lossy()),
            ("autofit-larger", "90%x90%"),
            ("cursor-autohide", "1000"),
            ("wayland-app-id", crate::APP_ID),
            ("x11-name", crate::APP_ID),
        ];
        for (k, v) in defaults {
            // Options an older mpv doesn't know (wayland-app-id) are skipped.
            if let Err(e) = handle.set_option(k, v) {
                log::debug!("{e}");
            }
        }
        // SAFETY: valid, not yet initialized handle.
        let r = unsafe { mpv_initialize(handle.0) };
        if r < 0 {
            return Err(format!("couldn't start mpv: {}", error_string(r)));
        }
        handle.observe(OBS_TIME, "time-pos", FORMAT_DOUBLE);
        handle.observe(OBS_DURATION, "duration", FORMAT_DOUBLE);
        handle.observe(OBS_PAUSE, "pause", FORMAT_FLAG);

        let shared = Arc::new(Shared {
            handle,
            state: Mutex::new(State::default()),
            pending_subs: Mutex::new(Vec::new()),
        });
        let events = shared.clone();
        std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || event_loop(events))
            .map_err(|e| e.to_string())?;
        Ok(Player { shared })
    }

    /// Starts playing `load`, replacing whatever was playing.
    pub fn load(&self, load: &Load) -> Result<(), String> {
        let h = &self.shared.handle;
        let start = load.start.filter(|s| *s > 1.0).map(|s| format!("{s:.3}"));
        h.set_property("start", start.as_deref().unwrap_or("none"))?;
        h.set_property("force-media-title", &load.title)?;
        h.set_property("http-header-fields", &load.headers.join(","))?;
        h.set_property("save-position-on-quit", if load.remember_position { "yes" } else { "no" })?;
        h.set_property("aid", &load.audio.map(|a| a.to_string()).unwrap_or_else(|| "auto".into()))?;
        let sid = match load.subtitle {
            Some(0) => "no".to_string(),
            Some(s) => s.to_string(),
            None => "auto".to_string(),
        };
        h.set_property("sid", &sid)?;
        *self.shared.pending_subs.lock().unwrap() = load.external_subs.clone();
        h.command(&["loadfile", &load.url, "replace"])?;
        h.set_property("pause", "no")
    }

    pub fn state(&self) -> State {
        self.shared.state.lock().unwrap().clone()
    }

    /// Closes the window and ends the player.
    pub fn quit(&self) {
        let _ = self.shared.handle.command(&["quit"]);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The event thread sees the shutdown, exits, and the handle is
        // destroyed when both have let go.
        self.quit();
    }
}

fn event_loop(shared: Arc<Shared>) {
    let h = shared.handle.0;
    loop {
        // SAFETY: the handle lives as long as `shared`; the event is valid
        // until the next wait_event on this thread.
        let ev = unsafe { &*mpv_wait_event(h, -1.0) };
        match ev.event_id {
            EVENT_SHUTDOWN => {
                shared.state.lock().unwrap().closed = true;
                return;
            }
            EVENT_START_FILE => {
                let mut s = shared.state.lock().unwrap();
                s.position = 0.0;
                s.duration = 0.0;
                s.error = None;
            }
            EVENT_FILE_LOADED => {
                let subs = std::mem::take(&mut *shared.pending_subs.lock().unwrap());
                for sub in subs {
                    let flag = if sub.select { "select" } else { "auto" };
                    if let Err(e) = shared.handle.command(&["sub-add", &sub.url, flag, &sub.title, &sub.lang]) {
                        log::warn!("{e}");
                    }
                }
                let mut s = shared.state.lock().unwrap();
                s.ended = None;
                s.loaded += 1;
            }
            EVENT_END_FILE if !ev.data.is_null() => {
                // SAFETY: END_FILE events carry an mpv_event_end_file.
                let end = unsafe { &*(ev.data as *const MpvEventEndFile) };
                let reason = match end.reason {
                    END_EOF => End::Eof,
                    END_ERROR => End::Error,
                    END_STOP | END_QUIT => End::Stopped,
                    _ => End::Stopped,
                };
                let mut s = shared.state.lock().unwrap();
                if reason == End::Error {
                    s.error = Some(error_string(end.error));
                }
                s.ended = Some(reason);
            }
            EVENT_PROPERTY_CHANGE if !ev.data.is_null() => {
                // SAFETY: PROPERTY_CHANGE events carry an mpv_event_property
                // whose data matches the format asked for.
                let prop = unsafe { &*(ev.data as *const MpvEventProperty) };
                let mut s = shared.state.lock().unwrap();
                match (ev.reply_userdata, prop.format) {
                    (OBS_TIME, FORMAT_DOUBLE) => s.position = unsafe { *(prop.data as *const f64) },
                    (OBS_DURATION, FORMAT_DOUBLE) => s.duration = unsafe { *(prop.data as *const f64) },
                    (OBS_PAUSE, FORMAT_FLAG) => s.paused = unsafe { *(prop.data as *const c_int) } != 0,
                    (_, FORMAT_NONE) => {}
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Maps Jellyfin's stream index (its position among all of a file's
/// streams) to mpv's track id (1-based, counted per type).
pub fn track_id(stream_indexes_of_type: &[i64], chosen: i64) -> Option<i64> {
    stream_indexes_of_type.iter().position(|i| *i == chosen).map(|p| p as i64 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_ids_count_per_type() {
        // Video 0, audio 1 and 2, subtitles 3 and 5 (4 is an attachment).
        assert_eq!(track_id(&[1, 2], 2), Some(2));
        assert_eq!(track_id(&[3, 5], 5), Some(2));
        assert_eq!(track_id(&[3, 5], 4), None);
    }
}
