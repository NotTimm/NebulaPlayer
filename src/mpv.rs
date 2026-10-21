//! A small binding to libmpv's client API, and the player built on it.
//!
//! Usually mpv draws into the app's window through its render API (see
//! `video.rs`), and the app draws the controls. In a window of its own
//! (macOS, or as a setting) mpv opens and draws its own window, with its
//! own on-screen controller and every key binding a user knows from mpv.
//! Either way the app tells it what to play and reads back where it is: a
//! background thread waits on mpv's events and keeps a [`State`] the UI
//! polls.
//!
//! Only the handful of client API calls the app needs are bound here, all
//! present since libmpv 1.x, so any distro's libmpv works. libmpv is
//! loaded when first needed (see [`Api`]), not linked.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Mutex, OnceLock};

use crate::video::{RenderContext, RenderParam};

// ── FFI ─────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct MpvHandle {
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
struct MpvEventClientMessage {
    num_args: c_int,
    args: *const *const c_char,
}

#[repr(C)]
struct MpvEventEndFile {
    reason: c_int,
    error: c_int,
}

const FORMAT_NONE: c_int = 0;
const FORMAT_STRING: c_int = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;

const EVENT_SHUTDOWN: c_int = 1;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_CLIENT_MESSAGE: c_int = 16;
const EVENT_PROPERTY_CHANGE: c_int = 22;

const END_EOF: c_int = 0;
const END_STOP: c_int = 2;
const END_QUIT: c_int = 3;
const END_ERROR: c_int = 4;

/// The libmpv functions used, looked up when first needed. Loading it at
/// run time (rather than linking it) means the app starts without libmpv
/// installed and can say so, and Windows needs no import library: the
/// DLL just sits next to the exe.
struct Api {
    create: unsafe extern "C" fn() -> *mut MpvHandle,
    initialize: unsafe extern "C" fn(*mut MpvHandle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(*mut MpvHandle),
    set_option_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int,
    observe_property: unsafe extern "C" fn(*mut MpvHandle, u64, *const c_char, c_int) -> c_int,
    wait_event: unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut MpvEvent,
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    client_api_version: unsafe extern "C" fn() -> std::ffi::c_ulong,
    /// The render API, for drawing into the app's window (libmpv 0.28+).
    render: Option<RenderApi>,
    /// Kept loaded for as long as the app runs.
    _lib: libloading::Library,
}

/// libmpv's render API: mpv draws video frames where the app asks.
pub struct RenderApi {
    pub create: unsafe extern "C" fn(*mut *mut RenderContext, *mut MpvHandle, *mut RenderParam) -> c_int,
    pub set_update_callback: unsafe extern "C" fn(*mut RenderContext, Option<extern "C" fn(*mut c_void)>, *mut c_void),
    pub update: unsafe extern "C" fn(*mut RenderContext) -> u64,
    pub render: unsafe extern "C" fn(*mut RenderContext, *mut RenderParam) -> c_int,
    pub report_swap: unsafe extern "C" fn(*mut RenderContext),
    pub free: unsafe extern "C" fn(*mut RenderContext),
}

/// Where libmpv is looked for, newest first.
#[cfg(target_os = "linux")]
const LIBRARY_NAMES: &[&str] = &["libmpv.so.2", "libmpv.so.1", "libmpv.so"];
#[cfg(windows)]
const LIBRARY_NAMES: &[&str] = &["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll"];
#[cfg(target_os = "macos")]
const LIBRARY_NAMES: &[&str] = &[
    "libmpv.2.dylib",
    "libmpv.dylib",
    "/opt/homebrew/lib/libmpv.dylib",
    "/usr/local/lib/libmpv.dylib",
];

/// What to tell someone whose system has no libmpv.
#[cfg(target_os = "linux")]
pub const INSTALL_HINT: &str =
    "Install libmpv to play videos: \"sudo dnf install mpv-libs\" on Fedora, \"sudo apt install libmpv2\" on Debian or Ubuntu.";
#[cfg(windows)]
pub const INSTALL_HINT: &str = "libmpv-2.dll is missing: it belongs next to nebula-player.exe. Reinstalling Nebula Player puts it back.";
#[cfg(target_os = "macos")]
pub const INSTALL_HINT: &str = "Install mpv to play videos: \"brew install mpv\".";

/// Candidates to load: on Windows the copy beside the exe first, by full
/// path, so no other folder on the search path can supply it.
fn library_paths() -> Vec<std::ffi::OsString> {
    let mut paths = Vec::new();
    #[cfg(windows)]
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
        paths.extend(LIBRARY_NAMES.iter().map(|name| dir.join(name).into_os_string()));
    }
    paths.extend(LIBRARY_NAMES.iter().map(std::ffi::OsString::from));
    paths
}

/// The Windows libmpv build imports vulkan-1.dll, which GPU drivers
/// install. Without one (no driver, some VMs) libmpv wouldn't load at all,
/// so the copy shipped in the `vulkan` folder is loaded first; Windows then
/// uses that module for libmpv's import. The system's loader always wins
/// when there is one, so a newer driver's loader is never shadowed.
#[cfg(windows)]
fn preload_vulkan() {
    use libloading::os::windows::{Library, LOAD_LIBRARY_SEARCH_SYSTEM32};
    // SAFETY: the Vulkan loader's constructors only set up its own state.
    if let Ok(lib) = unsafe { Library::load_with_flags("vulkan-1.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) } {
        std::mem::forget(lib);
        return;
    }
    let Some(bundled) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("vulkan").join("vulkan-1.dll"))) else {
        return;
    };
    // SAFETY: as above; kept loaded for as long as the app runs.
    match unsafe { Library::new(&bundled) } {
        Ok(lib) => {
            log::info!("no system Vulkan loader, using {}", bundled.display());
            std::mem::forget(lib);
        }
        Err(e) => log::warn!("no Vulkan loader: {e}"),
    }
}

fn load() -> Result<Api, String> {
    #[cfg(windows)]
    preload_vulkan();
    let mut errors = Vec::new();
    for name in library_paths() {
        // SAFETY: loading libmpv runs only its (benign) library constructors.
        let lib = match unsafe { libloading::Library::new(&name) } {
            Ok(lib) => lib,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
        };
        let name = name.to_string_lossy();
        // SAFETY: each symbol is looked up with libmpv's documented signature.
        unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get($name).map_err(|e| format!("{name}: {e}"))?
                };
            }
            macro_rules! opt {
                ($name:literal) => {
                    lib.get($name).ok().map(|s| *s)
                };
            }
            let render = (|| {
                Some(RenderApi {
                    create: opt!(b"mpv_render_context_create\0")?,
                    set_update_callback: opt!(b"mpv_render_context_set_update_callback\0")?,
                    update: opt!(b"mpv_render_context_update\0")?,
                    render: opt!(b"mpv_render_context_render\0")?,
                    report_swap: opt!(b"mpv_render_context_report_swap\0")?,
                    free: opt!(b"mpv_render_context_free\0")?,
                })
            })();
            let api = Api {
                render,
                create: sym!(b"mpv_create\0"),
                initialize: sym!(b"mpv_initialize\0"),
                terminate_destroy: sym!(b"mpv_terminate_destroy\0"),
                set_option_string: sym!(b"mpv_set_option_string\0"),
                set_property_string: sym!(b"mpv_set_property_string\0"),
                command: sym!(b"mpv_command\0"),
                observe_property: sym!(b"mpv_observe_property\0"),
                wait_event: sym!(b"mpv_wait_event\0"),
                error_string: sym!(b"mpv_error_string\0"),
                client_api_version: sym!(b"mpv_client_api_version\0"),
                _lib: lib,
            };
            log::info!("loaded {name}");
            return Ok(api);
        }
    }
    // The first error is the telling one (on Windows, the bundled copy's).
    log::warn!("libmpv not loaded: {}", errors.join("; "));
    Err(format!("{INSTALL_HINT} ({})", errors.first().map_or("", |e| e.as_str())))
}

fn api() -> Result<&'static Api, String> {
    static API: std::sync::OnceLock<Result<Api, String>> = std::sync::OnceLock::new();
    API.get_or_init(load).as_ref().map_err(|e| e.clone())
}

/// A handle only exists once the library has loaded.
fn loaded() -> &'static Api {
    api().expect("libmpv loaded before any handle")
}

fn error_string(code: c_int) -> String {
    // SAFETY: mpv returns a static string for every error code.
    unsafe { CStr::from_ptr((loaded().error_string)(code)) }.to_string_lossy().into_owned()
}

/// mpv's text for an error code.
pub fn error_text(code: c_int) -> String {
    error_string(code)
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// Which mpv plays videos, for the About page: libmpv's client API
/// version, or that it's the mpv program.
pub fn api_version() -> Option<String> {
    if use_ipc() {
        return Some("mpv program over IPC".into());
    }
    let api = api().ok()?;
    // SAFETY: a plain getter with no arguments.
    let v = unsafe { (api.client_api_version)() } as u64;
    Some(format!("libmpv API {}.{}", v >> 16, v & 0xffff))
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
        unsafe { (loaded().terminate_destroy)(self.0) };
    }
}

impl Handle {
    fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        // SAFETY: valid handle and NUL-terminated strings for the call.
        let r = unsafe { (loaded().set_option_string)(self.0, n.as_ptr(), v.as_ptr()) };
        if r < 0 {
            return Err(format!("mpv option {name}={value}: {}", error_string(r)));
        }
        Ok(())
    }

    fn set_property(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        // SAFETY: as above.
        let r = unsafe { (loaded().set_property_string)(self.0, n.as_ptr(), v.as_ptr()) };
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
        let r = unsafe { (loaded().command)(self.0, ptrs.as_ptr()) };
        if r < 0 {
            return Err(format!("mpv {}: {}", args.first().unwrap_or(&""), error_string(r)));
        }
        Ok(())
    }

    fn observe(&self, id: u64, name: &str, format: c_int) {
        let n = cstr(name);
        // SAFETY: valid handle and name.
        unsafe { (loaded().observe_property)(self.0, id, n.as_ptr(), format) };
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
    /// Counts presses of the skip key while a skip prompt is up.
    pub skip_presses: u64,
    /// The last error mpv reported for a file.
    pub error: Option<String>,
    /// 0–100 (and up to 130, mpv's boost).
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    /// Waiting for the network.
    pub buffering: bool,
    /// The file's audio, video and subtitle tracks.
    pub tracks: Vec<Track>,
}

/// One of a file's tracks, as mpv lists them.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Track {
    pub id: i64,
    /// "video", "audio" or "sub".
    #[serde(rename = "type")]
    pub kind: String,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
    pub external: bool,
    #[serde(rename = "demux-channel-count")]
    pub channels: Option<i64>,
}

impl Track {
    /// "English · 5.1 · AAC" or "Commentary (Japanese) · OPUS".
    pub fn label(&self) -> String {
        let lang = self.lang.as_deref().map(language_name);
        let mut parts: Vec<String> = Vec::new();
        match (&self.title, &lang) {
            (Some(t), Some(l)) if !t.to_lowercase().contains(&l.to_lowercase()) => parts.push(format!("{t} ({l})")),
            (Some(t), _) => parts.push(t.clone()),
            (None, Some(l)) => parts.push(l.clone()),
            (None, None) => parts.push(format!("Track {}", self.id)),
        }
        if self.kind == "audio" {
            match self.channels {
                Some(1) => parts.push("Mono".into()),
                Some(2) => parts.push("Stereo".into()),
                Some(6) => parts.push("5.1".into()),
                Some(8) => parts.push("7.1".into()),
                _ => {}
            }
        }
        if let (Some(c), false) = (&self.codec, self.kind == "sub" && self.title.is_some()) {
            parts.push(c.to_uppercase());
        }
        parts.join(" · ")
    }
}

/// English names for the language codes files usually carry.
fn language_name(code: &str) -> String {
    let name = match code {
        "eng" | "en" => "English",
        "jpn" | "ja" => "Japanese",
        "spa" | "es" => "Spanish",
        "fre" | "fra" | "fr" => "French",
        "ger" | "deu" | "de" => "German",
        "ita" | "it" => "Italian",
        "por" | "pt" => "Portuguese",
        "rus" | "ru" => "Russian",
        "chi" | "zho" | "zh" => "Chinese",
        "kor" | "ko" => "Korean",
        "dut" | "nld" | "nl" => "Dutch",
        "swe" | "sv" => "Swedish",
        "nor" | "no" => "Norwegian",
        "dan" | "da" => "Danish",
        "fin" | "fi" => "Finnish",
        "pol" | "pl" => "Polish",
        "tur" | "tr" => "Turkish",
        "ara" | "ar" => "Arabic",
        "hin" | "hi" => "Hindi",
        "und" => "Unknown",
        other => return other.to_string(),
    };
    name.to_string()
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
    /// Draw into the app's window rather than mpv's own.
    pub embed: bool,
    pub fullscreen: bool,
    pub hwdec: bool,
    /// Holds `mpv.conf`, `input.conf` and `scripts/` for this app's mpv.
    pub config_dir: std::path::PathBuf,
    pub watch_later_dir: std::path::PathBuf,
}

/// Talks to one mpv: through libmpv in this process, or (on macOS, where
/// libmpv can't open its own window) to the mpv program over its JSON IPC
/// socket. Both take mpv's input commands and option strings.
trait Backend: Send + Sync {
    fn command(&self, args: &[&str]) -> Result<(), String>;
    /// Sets an option or property from its string form.
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
}

impl Backend for Handle {
    fn command(&self, args: &[&str]) -> Result<(), String> {
        Handle::command(self, args)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.set_property(name, value)
    }
}

/// What happened in mpv, from either backend.
#[derive(Debug, PartialEq)]
enum Event {
    Shutdown,
    StartFile,
    FileLoaded,
    EndFile(End, Option<String>),
    Position(f64),
    Duration(f64),
    Paused(bool),
    Volume(f64),
    Muted(bool),
    Speed(f64),
    Buffering(bool),
    /// mpv's track-list, as JSON.
    Tracks(String),
    Message(Vec<String>),
}

struct Shared {
    backend: Box<dyn Backend>,
    /// Frames for the app's window, when mpv draws there.
    video: OnceLock<Arc<crate::video::Output>>,
    state: Mutex<State>,
    /// Subtitles for the file being loaded, added on FILE_LOADED.
    pending_subs: Mutex<Vec<ExternalSub>>,
}

impl Shared {
    fn new(backend: Box<dyn Backend>) -> Arc<Self> {
        Arc::new(Shared {
            backend,
            video: OnceLock::new(),
            state: Mutex::new(State { volume: 100.0, speed: 1.0, ..State::default() }),
            pending_subs: Mutex::new(Vec::new()),
        })
    }

    /// Updates the state for one event; false once mpv has shut down.
    fn handle(&self, event: Event) -> bool {
        match event {
            Event::Shutdown => {
                if let Some(video) = self.video.get() {
                    video.quit();
                }
                self.state.lock().unwrap().closed = true;
                return false;
            }
            Event::StartFile => {
                let mut s = self.state.lock().unwrap();
                s.position = 0.0;
                s.duration = 0.0;
                s.error = None;
            }
            Event::FileLoaded => {
                let subs = std::mem::take(&mut *self.pending_subs.lock().unwrap());
                for sub in subs {
                    let flag = if sub.select { "select" } else { "auto" };
                    if let Err(e) = self.backend.command(&["sub-add", &sub.url, flag, &sub.title, &sub.lang]) {
                        log::warn!("{e}");
                    }
                }
                let mut s = self.state.lock().unwrap();
                s.ended = None;
                s.loaded += 1;
            }
            Event::EndFile(reason, error) => {
                let mut s = self.state.lock().unwrap();
                if reason == End::Error {
                    s.error = error;
                }
                s.ended = Some(reason);
            }
            Event::Position(p) => self.state.lock().unwrap().position = p,
            Event::Duration(d) => self.state.lock().unwrap().duration = d,
            Event::Paused(p) => self.state.lock().unwrap().paused = p,
            Event::Volume(v) => self.state.lock().unwrap().volume = v,
            Event::Muted(m) => self.state.lock().unwrap().muted = m,
            Event::Speed(v) => self.state.lock().unwrap().speed = v,
            Event::Buffering(b) => self.state.lock().unwrap().buffering = b,
            Event::Tracks(json) => match serde_json::from_str::<Vec<Track>>(&json) {
                Ok(tracks) => self.state.lock().unwrap().tracks = tracks,
                Err(e) => log::warn!("mpv's track list: {e}"),
            },
            Event::Message(args) => {
                if args.first().map(String::as_str) == Some(SKIP_MESSAGE) {
                    self.state.lock().unwrap().skip_presses += 1;
                }
            }
        }
        true
    }
}

/// One mpv instance and its window. Dropping it closes the window.
pub struct Player {
    shared: Arc<Shared>,
}

/// The input section holding the skip keys (see [`Player::skip_keys`]).
const SKIP_SECTION: &str = "nebula-skip";
/// What the skip keys send back: a `script-message` reaches every client,
/// so it arrives here as a CLIENT_MESSAGE event.
const SKIP_MESSAGE: &str = "nebula-skip";
const SKIP_BINDINGS: &str = "s script-message nebula-skip\nS script-message nebula-skip\nENTER script-message nebula-skip\nKP_ENTER script-message nebula-skip";

const OBS_TIME: u64 = 1;
const OBS_DURATION: u64 = 2;
const OBS_PAUSE: u64 = 3;
const OBS_VOLUME: u64 = 4;
const OBS_MUTE: u64 = 5;
const OBS_TRACKS: u64 = 6;
const OBS_BUFFERING: u64 = 7;
const OBS_SPEED: u64 = 8;

/// What the player follows, with the format libmpv reports it in.
const OBSERVED: [(u64, &str, c_int); 8] = [
    (OBS_TIME, "time-pos", FORMAT_DOUBLE),
    (OBS_DURATION, "duration", FORMAT_DOUBLE),
    (OBS_PAUSE, "pause", FORMAT_FLAG),
    (OBS_VOLUME, "volume", FORMAT_DOUBLE),
    (OBS_MUTE, "mute", FORMAT_FLAG),
    (OBS_TRACKS, "track-list", FORMAT_STRING),
    (OBS_BUFFERING, "paused-for-cache", FORMAT_FLAG),
    (OBS_SPEED, "speed", FORMAT_DOUBLE),
];

/// mpv's options for this app. The user's own mpv.conf (read from
/// `config_dir`) overrides them. The last flag marks options only libmpv
/// gets: the mpv program refuses to start on one it doesn't know.
fn options(opts: &Options) -> Vec<(&'static str, String, bool)> {
    let o = |k: &'static str, v: &str| (k, v.to_string(), false);
    if opts.embed {
        return vec![
            o("config-dir", &opts.config_dir.to_string_lossy()),
            o("config", "yes"),
            o("terminal", "no"),
            // Frames go to the render API; the app draws the controls and
            // handles the keys.
            o("vo", "libmpv"),
            o("osc", "no"),
            o("input-default-bindings", "no"),
            o("input-vo-keyboard", "no"),
            o("idle", "yes"),
            o("keep-open", "no"),
            o("ytdl", "no"),
            // Decoded on the GPU and copied back: the off-screen renderer
            // has no display to share GPU frames through.
            o("hwdec", if opts.hwdec { "auto-copy-safe" } else { "no" }),
            o("watch-later-directory", &opts.watch_later_dir.to_string_lossy()),
        ];
    }
    vec![
        o("config-dir", &opts.config_dir.to_string_lossy()),
        o("config", "yes"),
        o("terminal", "no"),
        o("input-default-bindings", "yes"),
        o("input-vo-keyboard", "yes"),
        o("osc", "yes"),
        o("force-window", "yes"),
        // Stay open between files, so the next episode plays in the same
        // window; the app quits it when nothing follows.
        o("idle", "yes"),
        o("keep-open", "no"),
        o("ytdl", "no"),
        o("hwdec", if opts.hwdec { "auto-safe" } else { "no" }),
        o("fullscreen", if opts.fullscreen { "yes" } else { "no" }),
        o("title", "${media-title} — Nebula"),
        o("watch-later-directory", &opts.watch_later_dir.to_string_lossy()),
        o("autofit-larger", "90%x90%"),
        o("cursor-autohide", "1000"),
        ("wayland-app-id", crate::APP_ID.to_string(), true),
        ("x11-name", crate::APP_ID.to_string(), true),
    ]
}

/// Checks that mpv can be found and started, without opening a window:
/// libmpv loads, or the mpv program answers `--version`. For packaging
/// tests (`nebula-player --check-mpv`).
pub fn check() -> Result<String, String> {
    #[cfg(unix)]
    if use_ipc() {
        return ipc::check();
    }
    api()?;
    Ok(api_version().unwrap_or_default())
}

/// Whether videos can only play in mpv's own window: the mpv program
/// (macOS) has no way to draw into the app's.
pub fn separate_only() -> bool {
    use_ipc()
}

/// Whether mpv runs as its own program (macOS, or `NEBULA_MPV=ipc`).
fn use_ipc() -> bool {
    cfg!(target_os = "macos") || std::env::var("NEBULA_MPV").is_ok_and(|v| v == "ipc")
}

impl Player {
    pub fn new(opts: &Options) -> Result<Self, String> {
        let _ = std::fs::create_dir_all(&opts.config_dir);
        let _ = std::fs::create_dir_all(&opts.watch_later_dir);
        #[cfg(unix)]
        if use_ipc() {
            return ipc::start(opts).map(|shared| Player { shared });
        }
        Self::new_libmpv(opts)
    }

    fn new_libmpv(opts: &Options) -> Result<Self, String> {
        let api = api()?;
        // SAFETY: plain constructor; NULL means out of memory or a bad locale.
        let raw = unsafe { (api.create)() };
        if raw.is_null() {
            return Err("couldn't start mpv".into());
        }
        let handle = Handle(raw);
        for (k, v, _) in options(opts) {
            // Options an older mpv doesn't know (wayland-app-id) are skipped.
            if let Err(e) = handle.set_option(k, &v) {
                log::debug!("{e}");
            }
        }
        // SAFETY: valid, not yet initialized handle.
        let r = unsafe { (api.initialize)(handle.0) };
        if r < 0 {
            return Err(format!("couldn't start mpv: {}", error_string(r)));
        }
        for (id, name, format) in OBSERVED {
            handle.observe(id, name, format);
        }

        let raw = RawHandle(handle.0);
        let shared = Shared::new(Box::new(handle));
        if opts.embed {
            let render = api.render.as_ref().ok_or("this libmpv is too old to draw inside the app")?;
            let keep: Arc<dyn Send + Sync> = shared.clone();
            let video = crate::video::start(render, raw.0, keep)?;
            let _ = shared.video.set(video);
        }
        let events = shared.clone();
        std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || event_loop(events, raw))
            .map_err(|e| e.to_string())?;
        Ok(Player { shared })
    }

    /// Starts playing `load`, replacing whatever was playing.
    pub fn load(&self, load: &Load) -> Result<(), String> {
        let b = &self.shared.backend;
        let start = load.start.filter(|s| *s > 1.0).map(|s| format!("{s:.3}"));
        b.set("start", start.as_deref().unwrap_or("none"))?;
        b.set("force-media-title", &load.title)?;
        b.set("http-header-fields", &load.headers.join(","))?;
        b.set("save-position-on-quit", if load.remember_position { "yes" } else { "no" })?;
        b.set("aid", &load.audio.map(|a| a.to_string()).unwrap_or_else(|| "auto".into()))?;
        let sid = match load.subtitle {
            Some(0) => "no".to_string(),
            Some(s) => s.to_string(),
            None => "auto".to_string(),
        };
        b.set("sid", &sid)?;
        *self.shared.pending_subs.lock().unwrap() = load.external_subs.clone();
        b.command(&["loadfile", &load.url, "replace"])?;
        b.set("pause", "no")
    }

    pub fn state(&self) -> State {
        self.shared.state.lock().unwrap().clone()
    }

    /// Its frames, when mpv draws into the app's window.
    pub fn video(&self) -> Option<Arc<crate::video::Output>> {
        self.shared.video.get().cloned()
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = self.shared.backend.set("pause", if paused { "yes" } else { "no" });
    }

    /// Moves `seconds` forward (or back, when negative).
    pub fn seek_by(&self, seconds: f64) {
        let _ = self.shared.backend.command(&["seek", &format!("{seconds:.3}"), "relative"]);
    }

    pub fn set_volume(&self, volume: f64) {
        let _ = self.shared.backend.set("volume", &format!("{:.0}", volume.clamp(0.0, 130.0)));
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = self.shared.backend.set("mute", if muted { "yes" } else { "no" });
    }

    pub fn set_speed(&self, speed: f64) {
        let _ = self.shared.backend.set("speed", &format!("{speed:.2}"));
    }

    /// Picks the audio track with mpv's id.
    pub fn set_audio(&self, id: i64) {
        let _ = self.shared.backend.set("aid", &id.to_string());
    }

    /// Picks the subtitle track with mpv's id; `None` turns them off.
    pub fn set_subtitle(&self, id: Option<i64>) {
        let _ = self.shared.backend.set("sid", &id.map_or("no".to_string(), |i| i.to_string()));
    }

    /// Jumps to `seconds` into the file.
    pub fn seek(&self, seconds: f64) {
        let _ = self.shared.backend.command(&["seek", &format!("{seconds:.3}"), "absolute"]);
    }

    /// A message on mpv's on-screen display; an empty one clears it.
    pub fn show_text(&self, text: &str, millis: u32) {
        let _ = self.shared.backend.command(&["show-text", text, &millis.to_string()]);
    }

    /// Binds S and Enter to "skip" while a skip prompt is up, and back to
    /// whatever they were (S takes screenshots) when it's gone.
    pub fn skip_keys(&self, on: bool) {
        let b = &self.shared.backend;
        let result = if on {
            b.command(&["define-section", SKIP_SECTION, SKIP_BINDINGS, "force"])
                .and_then(|_| b.command(&["enable-section", SKIP_SECTION]))
        } else {
            b.command(&["disable-section", SKIP_SECTION])
        };
        if let Err(e) = result {
            log::warn!("{e}");
        }
    }

    /// Closes the window and ends the player.
    pub fn quit(&self) {
        let _ = self.shared.backend.command(&["quit"]);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The event thread sees the shutdown, exits, and the handle is
        // destroyed when both have let go.
        self.quit();
    }
}

/// The libmpv handle for the event thread, which `Shared` keeps alive.
struct RawHandle(*mut MpvHandle);

// SAFETY: see `Handle`.
unsafe impl Send for RawHandle {}

fn event_loop(shared: Arc<Shared>, h: RawHandle) {
    let wait_event = loaded().wait_event;
    loop {
        // SAFETY: the handle lives as long as `shared`; the event is valid
        // until the next wait_event on this thread.
        let ev = unsafe { &*wait_event(h.0, -1.0) };
        if ev.event_id != EVENT_PROPERTY_CHANGE {
            log::debug!("mpv event {}", ev.event_id);
        }
        let event = match ev.event_id {
            EVENT_SHUTDOWN => Event::Shutdown,
            EVENT_START_FILE => Event::StartFile,
            EVENT_FILE_LOADED => Event::FileLoaded,
            EVENT_END_FILE if !ev.data.is_null() => {
                // SAFETY: END_FILE events carry an mpv_event_end_file.
                let end = unsafe { &*(ev.data as *const MpvEventEndFile) };
                let reason = match end.reason {
                    END_EOF => End::Eof,
                    END_ERROR => End::Error,
                    END_STOP | END_QUIT => End::Stopped,
                    _ => End::Stopped,
                };
                Event::EndFile(reason, (reason == End::Error).then(|| error_string(end.error)))
            }
            EVENT_CLIENT_MESSAGE if !ev.data.is_null() => {
                // SAFETY: CLIENT_MESSAGE events carry num_args C strings.
                let msg = unsafe { &*(ev.data as *const MpvEventClientMessage) };
                let args = if msg.num_args > 0 && !msg.args.is_null() {
                    (0..msg.num_args as usize)
                        .filter_map(|i| {
                            let a = unsafe { *msg.args.add(i) };
                            (!a.is_null()).then(|| unsafe { CStr::from_ptr(a) }.to_string_lossy().into_owned())
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Event::Message(args)
            }
            EVENT_PROPERTY_CHANGE if !ev.data.is_null() => {
                // SAFETY: PROPERTY_CHANGE events carry an mpv_event_property
                // whose data matches the format asked for.
                let prop = unsafe { &*(ev.data as *const MpvEventProperty) };
                let double = || unsafe { *(prop.data as *const f64) };
                let flag = || unsafe { *(prop.data as *const c_int) } != 0;
                match (ev.reply_userdata, prop.format) {
                    (OBS_TIME, FORMAT_DOUBLE) => Event::Position(double()),
                    (OBS_DURATION, FORMAT_DOUBLE) => Event::Duration(double()),
                    (OBS_PAUSE, FORMAT_FLAG) => Event::Paused(flag()),
                    (OBS_VOLUME, FORMAT_DOUBLE) => Event::Volume(double()),
                    (OBS_MUTE, FORMAT_FLAG) => Event::Muted(flag()),
                    (OBS_SPEED, FORMAT_DOUBLE) => Event::Speed(double()),
                    (OBS_BUFFERING, FORMAT_FLAG) => Event::Buffering(flag()),
                    (OBS_TRACKS, FORMAT_STRING) => {
                        // SAFETY: STRING properties carry a char*.
                        let text = unsafe { *(prop.data as *const *const c_char) };
                        if text.is_null() {
                            continue;
                        }
                        Event::Tracks(unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned())
                    }
                    (_, FORMAT_NONE) | _ => continue,
                }
            }
            _ => continue,
        };
        if !shared.handle(event) {
            return;
        }
    }
}

/// The mpv program, driven over its JSON IPC socket.
#[cfg(unix)]
mod ipc {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::{options, Backend, End, Event, Options, Shared, OBSERVED};
    use super::{OBS_BUFFERING, OBS_DURATION, OBS_MUTE, OBS_PAUSE, OBS_SPEED, OBS_TIME, OBS_TRACKS, OBS_VOLUME};

    struct Ipc {
        socket: Mutex<UnixStream>,
        child: Mutex<Child>,
        next_id: AtomicU64,
    }

    impl Ipc {
        fn send(&self, command: serde_json::Value) -> Result<(), String> {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let mut line = serde_json::json!({ "command": command, "request_id": id }).to_string();
            line.push('\n');
            self.socket.lock().unwrap().write_all(line.as_bytes()).map_err(|e| format!("mpv: {e}"))
        }
    }

    impl Backend for Ipc {
        fn command(&self, args: &[&str]) -> Result<(), String> {
            self.send(serde_json::json!(args))
        }

        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            // `set` parses the value like a command line or mpv.conf does.
            self.send(serde_json::json!(["set", name, value]))
        }
    }

    impl Drop for Ipc {
        fn drop(&mut self) {
            if let Ok(mut child) = self.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// The mpv program to run: one bundled next to the app (the Mac .app
    /// carries its own), `NEBULA_MPV_BIN`, Homebrew's, or `mpv` on PATH.
    fn binary() -> PathBuf {
        if let Some(p) = std::env::var_os("NEBULA_MPV_BIN") {
            return PathBuf::from(p);
        }
        let bundled = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("mpv")));
        let candidates = bundled.into_iter().chain(
            ["/opt/homebrew/bin/mpv", "/usr/local/bin/mpv"].into_iter().map(PathBuf::from),
        );
        candidates.into_iter().find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("mpv"))
    }

    pub(super) fn check() -> Result<String, String> {
        let bin = binary();
        let out = Command::new(&bin)
            .arg("--version")
            .output()
            .map_err(|e| format!("couldn't run {}: {e}", bin.display()))?;
        let first = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string();
        if !out.status.success() || first.is_empty() {
            return Err(format!("{} --version failed ({})", bin.display(), out.status));
        }
        Ok(format!("{first} ({})", bin.display()))
    }

    pub(super) fn start(opts: &Options) -> Result<Arc<Shared>, String> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nebula-player-{}-{}.sock",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        let bin = binary();
        let mut cmd = Command::new(&bin);
        cmd.arg(format!("--input-ipc-server={}", path.display()));
        for (k, v, libmpv_only) in options(opts) {
            if !libmpv_only {
                cmd.arg(format!("--{k}={v}"));
            }
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start {}: {e}. {}", bin.display(), super::INSTALL_HINT))?;

        // mpv makes the socket as it starts up.
        let deadline = Instant::now() + Duration::from_secs(10);
        let socket = loop {
            match UnixStream::connect(&path) {
                Ok(s) => break s,
                Err(e) => {
                    if let Ok(Some(status)) = child.try_wait() {
                        return Err(format!("mpv exited at startup ({status})"));
                    }
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        return Err(format!("mpv didn't open its control socket: {e}"));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let reader = socket.try_clone().map_err(|e| e.to_string())?;
        let ipc = Ipc { socket: Mutex::new(socket), child: Mutex::new(child), next_id: AtomicU64::new(1) };
        for (id, name, _) in OBSERVED {
            ipc.send(serde_json::json!(["observe_property", id, name]))?;
        }
        let shared = Shared::new(Box::new(ipc));
        let events = shared.clone();
        std::thread::Builder::new()
            .name("mpv-ipc".into())
            .spawn(move || {
                for line in BufReader::new(reader).lines() {
                    let Ok(line) = line else { break };
                    if let Some(event) = parse(&line) {
                        if !events.handle(event) {
                            break;
                        }
                    }
                }
                // The socket closed: mpv has quit (or crashed).
                events.handle(Event::Shutdown);
                let _ = std::fs::remove_file(&path);
            })
            .map_err(|e| e.to_string())?;
        Ok(shared)
    }

    /// One line from mpv's socket, if it's an event the player follows.
    pub(super) fn parse(line: &str) -> Option<Event> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()).filter(|e| *e != "success") {
            log::warn!("mpv: {err}");
            return None;
        }
        Some(match v.get("event")?.as_str()? {
            "shutdown" => Event::Shutdown,
            "start-file" => Event::StartFile,
            "file-loaded" => Event::FileLoaded,
            "end-file" => {
                let reason = match v.get("reason").and_then(|r| r.as_str()) {
                    Some("eof") => End::Eof,
                    Some("error") => End::Error,
                    _ => End::Stopped,
                };
                let error = v.get("file_error").and_then(|e| e.as_str()).map(str::to_string);
                Event::EndFile(reason, error)
            }
            "client-message" => Event::Message(
                v.get("args")?.as_array()?.iter().filter_map(|a| a.as_str().map(str::to_string)).collect(),
            ),
            "property-change" => {
                let data = v.get("data")?;
                match v.get("id")?.as_u64()? {
                    OBS_TIME => Event::Position(data.as_f64()?),
                    OBS_DURATION => Event::Duration(data.as_f64()?),
                    OBS_PAUSE => Event::Paused(data.as_bool()?),
                    OBS_VOLUME => Event::Volume(data.as_f64()?),
                    OBS_MUTE => Event::Muted(data.as_bool()?),
                    OBS_SPEED => Event::Speed(data.as_f64()?),
                    OBS_BUFFERING => Event::Buffering(data.as_bool()?),
                    // Over IPC it arrives as JSON already: passed on as text,
                    // the same as libmpv's.
                    OBS_TRACKS => Event::Tracks(data.to_string()),
                    _ => return None,
                }
            }
            _ => return None,
        })
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

    #[cfg(unix)]
    #[test]
    fn ipc_events_parse() {
        use super::ipc::parse;
        assert_eq!(parse(r#"{"event":"property-change","id":1,"name":"time-pos","data":12.5}"#), Some(Event::Position(12.5)));
        assert_eq!(parse(r#"{"event":"property-change","id":3,"name":"pause","data":true}"#), Some(Event::Paused(true)));
        // Not known yet (no file): no event.
        assert_eq!(parse(r#"{"event":"property-change","id":2,"name":"duration"}"#), None);
        assert_eq!(parse(r#"{"event":"end-file","reason":"eof","playlist_entry_id":1}"#), Some(Event::EndFile(End::Eof, None)));
        assert_eq!(
            parse(r#"{"event":"end-file","reason":"error","file_error":"loading failed"}"#),
            Some(Event::EndFile(End::Error, Some("loading failed".into())))
        );
        assert_eq!(parse(r#"{"event":"client-message","args":["nebula-skip"]}"#), Some(Event::Message(vec!["nebula-skip".into()])));
        assert_eq!(parse(r#"{"request_id":4,"error":"success","data":null}"#), None);
    }

    #[test]
    fn track_list_parses() {
        let json = r#"[{"id":1,"type":"video","codec":"h264","selected":true},
            {"id":1,"type":"audio","lang":"eng","codec":"aac","demux-channel-count":6,"selected":true},
            {"id":2,"type":"audio","lang":"jpn","title":"Commentary","codec":"opus"},
            {"id":1,"type":"sub","lang":"eng","external":true,"title":"English SDH"}]"#;
        let tracks: Vec<Track> = serde_json::from_str(json).unwrap();
        assert_eq!(tracks.len(), 4);
        assert_eq!(tracks[1].label(), "English · 5.1 · AAC");
        assert_eq!(tracks[2].label(), "Commentary (Japanese) · OPUS");
        assert_eq!(tracks[3].label(), "English SDH");
        assert!(tracks[3].external && !tracks[3].selected);
    }

    #[test]
    fn track_ids_count_per_type() {
        // Video 0, audio 1 and 2, subtitles 3 and 5 (4 is an attachment).
        assert_eq!(track_id(&[1, 2], 2), Some(2));
        assert_eq!(track_id(&[3, 5], 5), Some(2));
        assert_eq!(track_id(&[3, 5], 4), None);
    }
}
