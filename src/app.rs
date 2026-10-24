//! The app: its pages, what they hold, and how messages move between them.
//!
//! Pages (`Page`) stack up as the user clicks into things, so Back returns
//! to where they were; picking something in the sidebar starts a new stack.
//! Data arrives from the server in messages tagged with what asked for it,
//! and is dropped when the user has moved on.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cosmic::app::{Core, Task};
use cosmic::dialog::file_chooser;
use cosmic::iced::{time, Length, Subscription};
use cosmic::widget::{self, icon, image::Handle, nav_bar};
use cosmic::{executor, Application, ApplicationExt, Element};

use crate::images::{self, Key};
use crate::jellyfin::{self, Account, Accounts, ImageRef, Item, Report};
use crate::mpv;
use crate::settings::{AccentChoice, Settings, SkipMode, ThemeChoice};
use cosmic::cosmic_theme::palette::Srgb;

mod dashboard;
mod details;
mod home;
mod library;
mod local;
mod player;
mod search;
mod settings_view;
mod signin;
mod syncplay;
mod widgets;
#[cfg_attr(not(windows), allow(dead_code))] // the title bar is only set on Windows
mod window_frame;

pub use library::{LibraryMsg, LibraryPage};
pub use player::{PlayerMsg, PlayerUi};
pub use local::LocalFile;
pub use syncplay::SyncMsg;
pub use dashboard::DashMsg;
use dashboard::Dashboard;
use syncplay::SyncState;

/// Image widths fetched for each use (about twice the size shown, for
/// HiDPI screens).
pub const POSTER_W: u32 = 320;
pub const LANDSCAPE_W: u32 = 560;
pub const BACKDROP_W: u32 = 1920;
pub const LOGO_W: u32 = 700;
pub const PERSON_W: u32 = 200;
/// How far the library grid scrolls between the reports that rebuild it.
const SCROLL_BUCKET: f32 = 160.0;

/// Items fetched per library page.
const PAGE_SIZE: usize = 100;

/// Where the sidebar can take the user.
#[derive(Clone, Debug)]
enum NavTarget {
    Home,
    Library(Item),
    Search,
    Local,
    Dashboard,
}

pub enum Page {
    Home,
    Library(Box<LibraryPage>),
    Details(Box<DetailsPage>),
    Search,
    Local,
    /// Administrators only (`App::admin`).
    Dashboard,
    SignIn,
}

#[derive(Default)]
pub struct Home {
    pub libraries: Vec<Item>,
    pub resume: Vec<Item>,
    pub next_up: Vec<Item>,
    /// Recently added, per library.
    pub latest: Vec<(Item, Vec<Item>)>,
    pub featured: Vec<Item>,
    pub featured_at: usize,
    pub loaded: bool,
    pub loading: bool,
}

/// Everything the home screen shows, fetched together.
#[derive(Clone, Debug, Default)]
pub struct HomeData {
    libraries: Vec<Item>,
    resume: Vec<Item>,
    next_up: Vec<Item>,
    latest: Vec<(Item, Vec<Item>)>,
    featured: Vec<Item>,
}

pub struct DetailsPage {
    pub item: Item,
    pub loading: bool,
    pub seasons: Vec<Item>,
    /// The season whose episodes are listed.
    pub season: Option<String>,
    pub episodes: Vec<Item>,
    pub episodes_loading: bool,
    /// Where the user is up to in a show.
    pub next_up: Option<Item>,
    pub similar: Vec<Item>,
}

/// What a details page fetches when it opens.
#[derive(Clone, Debug)]
pub struct DetailsData {
    item: Item,
    seasons: Vec<Item>,
    season: Option<String>,
    episodes: Vec<Item>,
    next_up: Option<Item>,
    similar: Vec<Item>,
}

#[derive(Default)]
pub struct SignIn {
    pub server: String,
    /// The server that answered: its address and name.
    pub found: Option<(String, String)>,
    pub quick_connect: Option<jellyfin::QuickConnect>,
    pub user: String,
    pub password: String,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct SearchState {
    pub term: String,
    pub searched: String,
    pub results: Vec<Item>,
    pub loading: bool,
}

/// The file playing in mpv right now.
struct Playback {
    /// Jellyfin's side of it; `None` for a local file.
    remote: Option<RemotePlay>,
    /// What the player's title bar shows.
    title: String,
    /// The episodes either side, for the player's buttons.
    previous: Option<Item>,
    next: Option<Item>,
    /// Thumbnails for the seek bar, when the server has made them.
    trickplay: Option<player::Trickplay>,
    chapters: Vec<jellyfin::Chapter>,
    /// `mpv::State::loaded` when this started, to tell its load from the
    /// previous file's.
    loaded_before: u64,
    started: bool,
    finished: bool,
    position: f64,
    paused: bool,
    last_report: Instant,
    /// Intros, credits and so on in this file, from the server.
    segments: Vec<jellyfin::Segment>,
    /// The segment whose skip prompt is showing.
    prompting: Option<usize>,
    /// Segments skipped automatically already (seeking back into one
    /// doesn't skip it again).
    skipped: HashSet<usize>,
    /// `mpv::State::skip_presses` already handled.
    skip_presses: u64,
    /// In a SyncPlay group: the group queue's entry this is.
    sync_pid: Option<String>,
    /// A skip that, in a group, becomes a group seek (seconds).
    group_seek: Option<f64>,
}

struct RemotePlay {
    account: Account,
    item: Item,
    media_source_id: String,
    play_session_id: String,
}

impl RemotePlay {
    fn report(&self, position: f64, paused: bool) -> Report {
        Report {
            item_id: self.item.id.clone(),
            media_source_id: self.media_source_id.clone(),
            play_session_id: self.play_session_id.clone(),
            position,
            paused,
        }
    }
}

/// An item ready to hand to mpv.
#[derive(Clone, Debug)]
pub struct PlayStart {
    item: Item,
    extras: jellyfin::SeekExtras,
    previous: Option<Item>,
    next: Option<Item>,
    info: jellyfin::PlaybackInfo,
    segments: Vec<jellyfin::Segment>,
    resume: bool,
    /// Where to start instead (seconds), for SyncPlay.
    start_at: Option<f64>,
    /// The SyncPlay queue entry this loads.
    sync_pid: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Message {
    // Sign-in
    ServerInput(String),
    Connect,
    ServerFound(Result<(String, String, bool), String>),
    QuickConnectStarted(Result<jellyfin::QuickConnect, String>),
    QuickConnectPoll,
    QuickConnectChecked(Result<bool, String>),
    UserInput(String),
    PasswordInput(String),
    PasswordSignIn,
    SignedIn(Result<Account, String>),
    AddAccount,
    SwitchAccount(String),
    SignOut,

    // Browsing
    RefreshHome,
    HomeLoaded(String, Result<HomeData, String>),
    FeaturedTick,
    FeaturedStep(i32),
    FeaturedPick(usize),
    ImageLoaded(Key, Result<images::Picture, String>),
    Open(Item),
    Library(LibraryMsg),
    DetailsLoaded(String, Result<Box<DetailsData>, String>),
    SeasonPick(String),
    EpisodesLoaded(String, Result<Vec<Item>, String>),
    TogglePlayed(Item),
    ToggleFavorite(Item),
    UserDataSaved(Result<(), String>),
    Back,
    /// The page scrolled: how far down it is (0–1) and in pixels.
    Scrolled(f32, f32),
    WindowOpened,
    /// Scrolls the page back to where it was (after the player).
    RestoreScroll(f32),
    SearchInput(String),
    SearchSubmit,
    SearchResults(String, Result<Vec<Item>, String>),

    // Playback
    Player(PlayerMsg),
    Play { item: Item, resume: bool },
    PlaybackReady(Result<Box<PlayStart>, String>),
    PlayerTick,
    NextEpisode(Result<Option<Item>, String>),
    Reported(Result<(), String>),
    Sync(SyncMsg),

    // Administrators
    Dashboard(DashMsg),
    AdminChecked(String, Result<bool, String>),

    // Local files
    OpenFile,
    FilePicked(PathBuf),
    AddFolder,
    FolderPicked(PathBuf),
    RemoveFolder(usize),
    LocalScanned(Vec<(PathBuf, Vec<LocalFile>)>),
    PlayLocal(PathBuf),

    // Settings
    ToggleSettings,
    SetTheme(usize),
    SetAccent(usize),
    /// The desktop's accent colour, now or changed.
    DesktopAccent(Srgb),
    /// A click on the page while the sidebar is open.
    CloseNav,
    SetFullscreen(bool),
    SetHwdec(bool),
    SetSeparateWindow(bool),
    SetAutoplay(bool),
    SetSkip(usize),
    OpenMpvConfig,

    DismissError,
    Cancelled,
}

pub struct App {
    core: Core,
    settings: Settings,
    accounts: Accounts,
    account: Option<Account>,
    nav: nav_bar::Model,
    page: Page,
    /// Pages under the current one, with how far down each was scrolled.
    back: Vec<(Page, f32)>,
    /// How far down the current page is scrolled (0–1).
    scroll_y: f32,
    /// Where to scroll the new page to, once this message is handled.
    snap: Option<f32>,
    home: Home,
    search: SearchState,
    signin: SignIn,
    local: Vec<(PathBuf, Vec<LocalFile>)>,
    local_scanning: bool,
    /// The desktop's accent colour, if it has one.
    desktop_accent: Option<Srgb>,
    images: HashMap<Key, images::Picture>,
    images_pending: HashSet<Key>,
    player: Option<mpv::Player>,
    playback: Option<Playback>,
    player_ui: PlayerUi,
    /// The window's width, in logical pixels.
    window_width: f32,
    /// A file from the command line, played once the window is up: mpv
    /// setting up its renderer while the app sets up its own has crashed
    /// graphics drivers.
    startup_file: Option<PathBuf>,
    /// The Windows title bar last set (`window_frame`); `None` until the
    /// window is up and has its icon.
    #[cfg_attr(not(windows), allow(dead_code))]
    frame: Option<window_frame::Frame>,
    /// SyncPlay (watch together).
    sync: SyncState,
    /// The signed-in user is a server administrator: the Dashboard shows.
    admin: bool,
    dashboard: Dashboard,
    error: Option<String>,
}

/// The scrollable every page's content sits in.
pub fn page_scroll_id() -> widget::Id {
    widget::Id::new("page-scroll")
}

pub(crate) fn tip<'a>(el: impl Into<Element<'a, Message>>, hint: impl Into<Cow<'a, str>>) -> Element<'a, Message> {
    widget::tooltip(el, widget::text(hint.into()), widget::tooltip::Position::Bottom).into()
}

impl App {
    pub fn image(&self, key: &Key) -> Option<&Handle> {
        self.images.get(key).map(|p| &p.handle)
    }

    pub fn picture(&self, key: &Key) -> Option<&images::Picture> {
        self.images.get(key)
    }

    /// Starts loading the images not already here or on the way.
    fn fetch_images(&mut self, keys: impl IntoIterator<Item = Key>) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        let mut tasks = Vec::new();
        for key in keys {
            if self.images.contains_key(&key) || !self.images_pending.insert(key.clone()) {
                continue;
            }
            let account = account.clone();
            tasks.push(cosmic::task::future(async move {
                let result = images::load(account, key.clone()).await;
                Message::ImageLoaded(key, result)
            }));
        }
        Task::batch(tasks)
    }

    fn poster_keys<'a>(items: impl IntoIterator<Item = &'a Item>) -> Vec<Key> {
        items.into_iter().filter_map(|i| i.poster()).map(|img| Key { img, width: POSTER_W }).collect()
    }

    fn landscape_keys<'a>(items: impl IntoIterator<Item = &'a Item>) -> Vec<Key> {
        items.into_iter().filter_map(|i| i.landscape()).map(|img| Key { img, width: LANDSCAPE_W }).collect()
    }

    fn key(img: ImageRef, width: u32) -> Key {
        Key { img, width }
    }

    // ── Navigation ───────────────────────────────────────────────────────────

    fn rebuild_nav(&mut self) {
        self.nav.clear();
        self.nav.insert().text("Home").icon(icon::from_name("go-home-symbolic")).data(NavTarget::Home).activate();
        for lib in &self.home.libraries {
            let icon_name = match lib.collection_type.as_deref() {
                Some("movies") => "video-x-generic-symbolic",
                Some("tvshows") => "video-display-symbolic",
                Some("boxsets") => "view-grid-symbolic",
                _ => "folder-videos-symbolic",
            };
            self.nav
                .insert()
                .text(lib.name.clone())
                .icon(icon::from_name(icon_name))
                .data(NavTarget::Library(lib.clone()));
        }
        self.nav.insert().text("Search").icon(icon::from_name("system-search-symbolic")).data(NavTarget::Search);
        self.nav.insert().text("Local Files").icon(icon::from_name("folder-symbolic")).data(NavTarget::Local);
        if self.admin {
            self.nav
                .insert()
                .text("Dashboard")
                .icon(icon::from_name("preferences-system-symbolic"))
                .data(NavTarget::Dashboard);
        }
        self.select_nav();
    }

    /// Highlights the sidebar entry for the page the user is on (the
    /// bottom of the stack, where the sidebar took them).
    fn select_nav(&mut self) {
        let root = self.back.first().map(|(p, _)| p).unwrap_or(&self.page);
        let wanted = |t: &NavTarget| match (t, root) {
            (NavTarget::Home, Page::Home) => true,
            (NavTarget::Search, Page::Search) => true,
            (NavTarget::Local, Page::Local) => true,
            (NavTarget::Dashboard, Page::Dashboard) => true,
            (NavTarget::Library(lib), Page::Library(p)) => lib.id == p.query.parent.id,
            _ => false,
        };
        let found = self.nav.iter().find(|id| self.nav.data::<NavTarget>(*id).is_some_and(wanted));
        if let Some(id) = found {
            self.nav.activate(id);
        }
    }

    /// Opens a page on top of the current one; Back returns.
    fn push(&mut self, page: Page) {
        let old = std::mem::replace(&mut self.page, page);
        if !matches!(old, Page::SignIn) {
            self.back.push((old, self.scroll_y));
        }
        self.scroll_to(0.0);
    }

    /// Opens a sidebar page: a fresh stack.
    fn go(&mut self, page: Page) {
        self.back.clear();
        self.page = page;
        self.scroll_to(0.0);
    }

    /// Every page scrolls in the same widget, so a new page starts where
    /// the last one was unless told otherwise.
    fn scroll_to(&mut self, y: f32) {
        self.scroll_y = y;
        self.snap = Some(y);
    }

    fn with_snap(&mut self, task: Task<Message>) -> Task<Message> {
        match self.snap.take() {
            Some(y) => Task::batch([
                task,
                cosmic::iced::widget::scrollable::snap_to(
                    page_scroll_id(),
                    cosmic::iced::widget::scrollable::RelativeOffset { x: None, y: Some(y) },
                ),
            ]),
            None => task,
        }
    }

    fn open_item(&mut self, item: Item) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        if item.opens_library() {
            return self.open_library(item, false);
        }
        // A season opens its show, at that season.
        let (target, season) = match (item.kind.as_str(), &item.series_id) {
            ("Season", Some(series)) => {
                (Item { id: series.clone(), kind: "Series".into(), name: item.series_name.clone().unwrap_or_default(), ..Default::default() }, Some(item.id.clone()))
            }
            _ => (item, None),
        };
        let id = target.id.clone();
        self.push(Page::Details(Box::new(DetailsPage {
            item: target,
            loading: true,
            seasons: Vec::new(),
            season: None,
            episodes: Vec::new(),
            episodes_loading: false,
            next_up: None,
            similar: Vec::new(),
        })));
        cosmic::task::future(async move {
            let result = load_details(&account, &id, season).await.map(Box::new);
            Message::DetailsLoaded(id, result)
        })
    }

    fn refresh_home(&mut self) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        self.home.loading = true;
        let key = account.key();
        cosmic::task::future(async move {
            let result = load_home(&account).await;
            Message::HomeLoaded(key, result)
        })
    }

    /// Re-fetches the open details page (after playing or marking watched).
    fn refresh_details(&mut self) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        let Page::Details(d) = &self.page else { return Task::none() };
        let id = d.item.id.clone();
        let season = d.season.clone();
        cosmic::task::future(async move {
            let result = load_details(&account, &id, season).await.map(Box::new);
            Message::DetailsLoaded(id, result)
        })
    }

    fn use_account(&mut self, account: Option<Account>) -> Task<Message> {
        self.settings.account = account.as_ref().map(|a| a.key());
        self.settings.save();
        // SyncPlay groups belong to the session that joined them.
        self.sync = SyncState::new();
        self.admin = false;
        self.dashboard = Dashboard::default();
        self.account = account;
        self.home = Home::default();
        self.back.clear();
        self.scroll_to(0.0);
        self.images.clear();
        self.images_pending.clear();
        self.rebuild_nav();
        if let Some(account) = self.account.clone() {
            self.page = Page::Home;
            // Whether to offer the Dashboard.
            let key = account.key();
            let admin = cosmic::task::future(async move {
                Message::AdminChecked(key, crate::admin::is_admin(&account).await)
            });
            Task::batch([self.refresh_home(), admin])
        } else {
            self.signin = SignIn::default();
            self.page = Page::SignIn;
            Task::none()
        }
    }

    // ── Playback ─────────────────────────────────────────────────────────────

    fn player_options(&self) -> mpv::Options {
        mpv::Options {
            embed: !self.settings.separate_window && !mpv::separate_only(),
            fullscreen: self.settings.fullscreen,
            hwdec: self.settings.hwdec,
            config_dir: crate::platform::mpv_config_dir(),
            watch_later_dir: crate::platform::data_dir().join("watch_later"),
        }
    }

    /// The running player, or a new one if there's none (or mpv's window
    /// was closed). When it draws into this window, the window switches to
    /// the player (`entered`).
    fn ensure_player(&mut self) -> Result<(&mpv::Player, Task<Message>), String> {
        let opts = self.player_options();
        if self.player.as_ref().is_some_and(|p| p.state().closed || p.video().is_some() != opts.embed) {
            self.player = None;
        }
        let mut entered = Task::none();
        if self.player.is_none() {
            let player = match mpv::Player::new(&opts) {
                Err(e) if opts.embed => {
                    log::warn!("can't play inside the window ({e}); using mpv's own");
                    mpv::Player::new(&mpv::Options { embed: false, ..opts })?
                }
                other => other?,
            };
            let inside = player.video().is_some();
            self.player = Some(player);
            if inside {
                entered = self.enter_player();
            }
        }
        Ok((self.player.as_ref().unwrap(), entered))
    }

    /// Ends playback from the player screen: reports where it stopped,
    /// closes mpv, and returns to the page underneath.
    fn stop_playback(&mut self) -> Task<Message> {
        let stopped = self.end_playback();
        self.player = None;
        let left = self.leave_player();
        Task::batch([stopped, left, self.refresh_home(), self.refresh_details()])
    }

    /// Skips the segment whose Skip button is up.
    fn skip_now(&mut self) {
        let (Some(player), Some(pb)) = (&self.player, &mut self.playback) else { return };
        let Some(i) = pb.prompting.take() else { return };
        let state = player.state();
        let seg = pb.segments[i];
        let grouped = pb.sync_pid.is_some();
        skip_to(player, &seg, &state, grouped.then_some(&mut pb.group_seek));
    }

    /// Tells the server the current file stopped (if it's a server's).
    fn end_playback(&mut self) -> Task<Message> {
        let Some(pb) = self.playback.take() else { return Task::none() };
        let Some(remote) = pb.remote else { return Task::none() };
        if !pb.started {
            return Task::none();
        }
        let report = remote.report(pb.position, pb.paused);
        let account = remote.account;
        cosmic::task::future(async move { Message::Reported(jellyfin::report_stopped(&account, report).await) })
    }

    /// Fetches what playing `item` needs (resolving a show or season to its
    /// episode), then plays it. An item with only its id is fetched first.
    fn prepare_play(&mut self, item: Item, resume: bool, start_at: Option<f64>, sync_pid: Option<String>) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        cosmic::task::future(async move {
            let result = async {
                let item = if item.kind.is_empty() { jellyfin::item(&account, &item.id).await? } else { item };
                // A show or season plays where the user is up to.
                let item = match item.kind.as_str() {
                    "Series" => jellyfin::series_start(&account, &item.id)
                        .await?
                        .ok_or_else(|| format!("{} has no episodes.", item.name))?,
                    "Season" => {
                        let series = item.series_id.clone().unwrap_or_default();
                        let eps = jellyfin::episodes(&account, &series, &item.id).await?;
                        eps.iter()
                            .find(|e| !e.played())
                            .or(eps.first())
                            .cloned()
                            .ok_or_else(|| format!("{} has no episodes.", item.name))?
                    }
                    _ => item,
                };
                let neighbours = async {
                    match (&item.series_id, item.is_episode()) {
                        (Some(series), true) => {
                            jellyfin::episode_neighbours(&account, series, &item.id).await.unwrap_or_default()
                        }
                        _ => (None, None),
                    }
                };
                let (info, segments, (previous, next), extras) = tokio::join!(
                    jellyfin::playback_info(&account, &item.id),
                    jellyfin::segments(&account, &item.id),
                    neighbours,
                    jellyfin::seek_extras(&account, &item.id),
                );
                Ok(Box::new(PlayStart { item, extras, previous, next, info: info?, segments, resume, start_at, sync_pid }))
            }
            .await;
            Message::PlaybackReady(result)
        })
    }

    fn start_remote(&mut self, start: PlayStart) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        let Some(source) = start.info.media_sources.first().cloned() else {
            self.error = Some(format!("{} has no file to play.", start.item.name));
            return Task::none();
        };
        let stopped = self.end_playback();
        let play_session_id = start.info.play_session_id.clone().unwrap_or_default();
        let (audio, subtitle) = source.track_choice();
        let load = mpv::Load {
            url: account.stream_url(&start.item.id, &source, &play_session_id),
            title: media_title(&start.item),
            start: start.start_at.or_else(|| start.resume.then(|| start.item.resume_seconds())),
            headers: account.stream_headers(),
            audio,
            subtitle,
            external_subs: source.external_subs(&account, &start.item.id),
            remember_position: false,
        };
        let (loaded_before, entered) = match self.ensure_player() {
            Ok((p, entered)) => {
                let before = p.state().loaded;
                // A SyncPlay item waits paused for the group to start.
                p.set_paused(start.sync_pid.is_some());
                if let Err(e) = p.load(&load) {
                    self.error = Some(e);
                    return stopped;
                }
                // A skip prompt left up by the last file.
                p.skip_keys(false);
                (before, entered)
            }
            Err(e) => {
                self.error = Some(e);
                return stopped;
            }
        };
        // Thumbnails about a fifth of the window across, as jellyfin-web
        // picks them for the screen.
        let max_width = ((self.window_width * self.core.scale_factor() * 0.2) as u32).max(240);
        let trickplay = start
            .extras
            .trickplay(&source.id, max_width)
            .map(|info| player::Trickplay::new(start.item.id.clone(), source.id.clone(), info));
        self.playback = Some(Playback {
            title: load.title.clone(),
            previous: start.previous,
            next: start.next,
            trickplay,
            chapters: start.extras.chapters,
            remote: Some(RemotePlay {
                account,
                item: start.item,
                media_source_id: source.id,
                play_session_id,
            }),
            loaded_before,
            started: false,
            finished: false,
            position: load.start.unwrap_or(0.0),
            paused: false,
            last_report: Instant::now(),
            segments: start.segments,
            prompting: None,
            skipped: HashSet::new(),
            skip_presses: self.player.as_ref().map(|p| p.state().skip_presses).unwrap_or(0),
            sync_pid: start.sync_pid,
            group_seek: None,
        });
        Task::batch([stopped, entered])
    }

    fn play_local(&mut self, path: PathBuf) -> Task<Message> {
        let stopped = self.end_playback();
        let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let load = mpv::Load {
            url: path.to_string_lossy().into_owned(),
            title,
            remember_position: true,
            ..Default::default()
        };
        match self.ensure_player() {
            Ok((p, entered)) => {
                let loaded_before = p.state().loaded;
                if let Err(e) = p.load(&load) {
                    self.error = Some(e);
                    return stopped;
                }
                p.skip_keys(false);
                let skip_presses = p.state().skip_presses;
                self.playback = Some(Playback {
                    title: load.title.clone(),
                    previous: None,
                    next: None,
                    trickplay: None,
                    chapters: Vec::new(),
                    remote: None,
                    loaded_before,
                    started: false,
                    finished: false,
                    position: 0.0,
                    paused: false,
                    last_report: Instant::now(),
                    segments: Vec::new(),
                    prompting: None,
                    skipped: HashSet::new(),
                    skip_presses,
                    sync_pid: None,
                    group_seek: None,
                });
                return Task::batch([stopped, entered]);
            }
            Err(e) => self.error = Some(e),
        }
        stopped
    }

    /// Follows mpv: reports to Jellyfin, plays on into the next episode,
    /// and notices the window closing.
    fn player_tick(&mut self) -> Task<Message> {
        let Some(player) = &self.player else { return Task::none() };
        let state = player.state();
        let inside = player.video().is_some();
        if inside {
            self.player_ui.state = state.clone();
            self.player_idle_check();
        }
        if state.closed {
            self.player = None;
            let stopped = self.end_playback();
            let left = if inside { self.leave_player() } else { Task::none() };
            // Continue Watching and the page's progress bars have changed.
            return Task::batch([stopped, left, self.refresh_home(), self.refresh_details()]);
        }
        let Some(pb) = &mut self.playback else { return self.sync_tick(&state) };
        let grouped = pb.sync_pid.is_some();
        if state.loaded > pb.loaded_before && state.ended.is_none() && state.duration > 0.0 {
            pb.position = state.position;
        }
        let paused_changed = pb.paused != state.paused;
        pb.paused = state.paused;
        if state.loaded > pb.loaded_before && state.ended.is_none() {
            if let Some(player) = &self.player {
                skip_segments(player, pb, &state, self.settings.skip, inside);
            }
        }

        let mut tasks = Vec::new();
        if let Some(remote) = &pb.remote {
            let account = remote.account.clone();
            if !pb.started && state.loaded > pb.loaded_before {
                pb.started = true;
                pb.last_report = Instant::now();
                let r = remote.report(pb.position, pb.paused);
                tasks.push(cosmic::task::future(async move {
                    Message::Reported(jellyfin::report_started(&account, r).await)
                }));
            } else if pb.started && !pb.finished && (paused_changed || pb.last_report.elapsed() >= Duration::from_secs(10)) {
                pb.last_report = Instant::now();
                let r = remote.report(pb.position, pb.paused);
                tasks.push(cosmic::task::future(async move {
                    Message::Reported(jellyfin::report_progress(&account, r).await)
                }));
            }
        }

        if pb.started || pb.remote.is_none() {
            // mpv can report an error at the end of a file it played all
            // the way through (an audio device that failed to open, say):
            // that still counts as watched.
            let played_out = state.duration > 0.0 && pb.position >= state.duration - 3.0;
            let ended = match state.ended {
                Some(mpv::End::Error) if played_out => Some(mpv::End::Eof),
                e => e,
            };
            match ended {
                Some(mpv::End::Eof) if !pb.finished && state.loaded > pb.loaded_before => {
                    pb.finished = true;
                    // Played to the end: report the end, so it's marked watched.
                    if state.duration > 0.0 {
                        pb.position = state.duration;
                    }
                    let next = pb.remote.as_ref().and_then(|r| {
                        let item = &r.item;
                        match (&item.series_id, item.is_episode() && self.settings.autoplay_next) {
                            (Some(series), true) => Some((r.account.clone(), series.clone(), item.id.clone())),
                            _ => None,
                        }
                    });
                    tasks.push(self.end_playback());
                    // In a group, everyone asks for the next item; the
                    // server acts on the first ask.
                    let next = if grouped && self.sync_active() {
                        if self.group_has_next() && self.settings.autoplay_next {
                            tasks.push(self.group_next(true));
                            return Task::batch(tasks);
                        }
                        None
                    } else {
                        next
                    };
                    match next {
                        Some((account, series, episode)) => tasks.push(cosmic::task::future(async move {
                            Message::NextEpisode(jellyfin::episode_after(&account, &series, &episode).await)
                        })),
                        None => {
                            if let Some(p) = &self.player {
                                p.quit();
                            }
                        }
                    }
                }
                Some(mpv::End::Error) if !pb.finished && state.loaded >= pb.loaded_before => {
                    pb.finished = true;
                    self.error = Some(format!(
                        "mpv couldn't play this: {}",
                        state.error.clone().unwrap_or_else(|| "unknown error".into())
                    ));
                    tasks.push(self.end_playback());
                    if let Some(p) = &self.player {
                        p.quit();
                    }
                }
                _ => {}
            }
        }
        if let Some(target) = self.playback.as_mut().and_then(|pb| pb.group_seek.take()) {
            tasks.push(self.group_seek(target));
        }
        tasks.push(self.sync_tick(&state));
        Task::batch(tasks)
    }

    /// Windows: gives the title bar the app's icon and the theme's
    /// colours, once the window is up and again whenever they change.
    #[cfg(windows)]
    fn sync_frame(&mut self) -> Task<Message> {
        let theme = crate::theme::build(self.settings.theme, self.settings.accent, self.desktop_accent);
        let frame = window_frame::Frame::of(&theme);
        if self.frame == Some(frame) {
            return Task::none();
        }
        let Some(id) = self.core.main_window_id() else { return Task::none() };
        let icon = self.frame.is_none();
        self.frame = Some(frame);
        window_frame::apply(id, frame, icon)
    }

    fn apply_theme(&self) -> Task<Message> {
        cosmic::command::set_theme(crate::theme::build(self.settings.theme, self.settings.accent, self.desktop_accent))
    }

    fn scan_local(&mut self) -> Task<Message> {
        let folders = self.settings.local_folders.clone();
        self.local_scanning = true;
        cosmic::task::future(async move {
            let found = tokio::task::spawn_blocking(move || local::scan(&folders)).await.unwrap_or_default();
            Message::LocalScanned(found)
        })
    }
}

/// Jumps past a segment.
/// Skips a segment; in a SyncPlay group (`group_seek`) the skip is a seek
/// for the whole group, as jellyfin-web's skip button does.
fn skip_to(player: &mpv::Player, seg: &jellyfin::Segment, state: &mpv::State, group_seek: Option<&mut Option<f64>>) {
    // Credits that run to the end skip to the end, so the next episode
    // starts.
    let end = if state.duration > 0.0 { seg.end.min(state.duration) } else { seg.end };
    match group_seek {
        Some(seek) => *seek = Some(end),
        None => player.seek(end),
    }
    player.show_text(&format!("Skipped {}", seg.kind.label().to_lowercase()), 1500);
}

/// Offers to skip (or skips) the intro, credits, recap or preview the
/// position is in, and acts on the skip key. Inside the app's window
/// (`inside`) the offer is the player's Skip button; in mpv's own window
/// it's a message on mpv's screen, with S and Enter bound to skip.
fn skip_segments(player: &mpv::Player, pb: &mut Playback, state: &mpv::State, mode: SkipMode, inside: bool) {
    // The last half second of a segment isn't worth a prompt.
    let current = pb.segments.iter().position(|s| state.position >= s.start && state.position < s.end - 0.5);
    let grouped = pb.sync_pid.is_some();
    let skip_to = |player: &mpv::Player, seg: &jellyfin::Segment, group_seek: &mut Option<f64>| {
        skip_to(player, seg, state, grouped.then_some(group_seek))
    };

    let pressed = state.skip_presses > pb.skip_presses;
    pb.skip_presses = state.skip_presses;

    if pb.prompting.is_some() && pb.prompting != current {
        // Left the segment (watched it, or seeked away).
        if !inside {
            player.skip_keys(false);
            player.show_text("", 1);
        }
        pb.prompting = None;
    }
    let Some(i) = current else { return };
    let seg = pb.segments[i];
    match mode {
        SkipMode::Off => {}
        SkipMode::Auto => {
            if pb.skipped.insert(i) {
                skip_to(player, &seg, &mut pb.group_seek);
            }
        }
        SkipMode::Ask => {
            if pressed && pb.prompting == Some(i) {
                player.skip_keys(false);
                pb.prompting = None;
                skip_to(player, &seg, &mut pb.group_seek);
            } else if pb.prompting.is_none() && inside {
                pb.prompting = Some(i);
            } else if pb.prompting.is_none() {
                pb.prompting = Some(i);
                player.skip_keys(true);
                // Shown until the segment ends (or for 15 s at most); it's
                // cleared early if the user skips or leaves it.
                let millis = ((seg.end - state.position).clamp(1.0, 15.0) * 1000.0) as u32;
                player.show_text(&format!("Skip {}: press S or Enter", seg.kind.label()), millis);
            }
        }
    }
}

/// What mpv's title bar shows: "Show — S1:E2 — Episode" or "Movie (2021)".
fn media_title(item: &Item) -> String {
    match (&item.series_name, item.episode_code()) {
        (Some(series), Some(code)) => format!("{series} — {code} — {}", item.name),
        _ => match item.production_year {
            Some(y) => format!("{} ({y})", item.name),
            None => item.name.clone(),
        },
    }
}

async fn load_home(account: &Account) -> Result<HomeData, String> {
    use cosmic::iced::futures::future::join_all;

    let (libraries, resume, next_up, featured) = tokio::join!(
        jellyfin::libraries(account),
        jellyfin::resume(account, 16),
        jellyfin::next_up(account, 16, None),
        jellyfin::featured(account, 8),
    );
    let libraries = libraries?;
    let latest = join_all(libraries.iter().filter(|l| l.collection_type.as_deref() != Some("boxsets")).map(|lib| async move {
        let items = jellyfin::latest(account, &lib.id, 16).await.unwrap_or_default();
        (lib.clone(), items)
    }))
    .await
    .into_iter()
    .filter(|(_, items)| !items.is_empty())
    .collect();
    Ok(HomeData {
        libraries,
        resume: resume.unwrap_or_default(),
        next_up: next_up.unwrap_or_default(),
        latest,
        featured: featured.unwrap_or_default(),
    })
}

async fn load_details(account: &Account, id: &str, season: Option<String>) -> Result<DetailsData, String> {
    let (item, similar) = tokio::join!(jellyfin::item(account, id), jellyfin::similar(account, id, 12));
    let item = item?;
    let mut data = DetailsData {
        seasons: Vec::new(),
        season: None,
        episodes: Vec::new(),
        next_up: None,
        similar: if item.is_episode() { Vec::new() } else { similar.unwrap_or_default() },
        item,
    };
    if data.item.is_series() {
        let (seasons, next) = tokio::join!(jellyfin::seasons(account, id), jellyfin::next_up(account, 1, Some(id)));
        data.seasons = seasons?;
        data.next_up = next.ok().and_then(|n| n.into_iter().next());
        // The season asked for, else the one the user is up to, else the
        // first.
        let pick = season
            .filter(|s| data.seasons.iter().any(|x| &x.id == s))
            .or_else(|| data.next_up.as_ref().and_then(|n| n.season_id.clone()))
            .or_else(|| data.seasons.first().map(|s| s.id.clone()));
        if let Some(season_id) = pick {
            data.episodes = jellyfin::episodes(account, id, &season_id).await.unwrap_or_default();
            data.season = Some(season_id);
        }
    }
    Ok(data)
}

impl Application for App {
    type Executor = executor::Default;
    type Flags = Option<PathBuf>;
    type Message = Message;
    const APP_ID: &'static str = crate::APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let settings = Settings::load();
        let accounts = Accounts::load();
        let account = settings
            .account
            .as_deref()
            .and_then(|k| accounts.find(k))
            .or_else(|| accounts.accounts.first())
            .cloned();
        let mut app = App {
            core,
            settings,
            accounts,
            account: None,
            nav: nav_bar::Model::default(),
            page: Page::Home,
            back: Vec::new(),
            scroll_y: 0.0,
            snap: None,
            home: Home::default(),
            search: SearchState::default(),
            signin: SignIn::default(),
            local: Vec::new(),
            local_scanning: false,
            desktop_accent: crate::theme::desktop_accent(),
            images: HashMap::new(),
            images_pending: HashSet::new(),
            player: None,
            playback: None,
            player_ui: PlayerUi::default(),
            window_width: 1280.0,
            startup_file: flags,
            frame: None,
            sync: SyncState::new(),
            admin: false,
            dashboard: Dashboard::default(),
            error: None,
        };
        app.set_header_title("Nebula Player".into());
        if crate::platform::native_titlebar() {
            // The system title bar has the window buttons and title, so
            // libcosmic's header bar becomes a plain toolbar.
            let window = &mut app.core.window;
            window.show_close = false;
            window.show_maximize = false;
            window.show_minimize = false;
            window.sharp_corners = true;
        }
        // The sidebar opens from the header's button, over the page.
        app.core.nav_bar_set_toggled(false);
        // Pages reach the window's edges (see theme::page).
        app.core.window.content_container = false;
        let title = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Nebula Player".into(), id),
            None => Task::none(),
        };
        let tasks = vec![title, app.use_account(account), app.scan_local()];
        (app, Task::batch(tasks))
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        match self.page {
            Page::SignIn => None,
            _ if self.playing_inside() => None,
            _ => Some(&self.nav),
        }
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Self::Message> {
        self.nav.activate(id);
        self.core.nav_bar_set_toggled(false);
        let task = match self.nav.data::<NavTarget>(id).cloned() {
            Some(NavTarget::Home) => {
                self.go(Page::Home);
                self.refresh_home()
            }
            Some(NavTarget::Library(lib)) => self.open_library(lib, true),
            Some(NavTarget::Search) => {
                self.go(Page::Search);
                widget::text_input::focus(search::INPUT_ID.clone())
            }
            Some(NavTarget::Local) => {
                self.go(Page::Local);
                self.scan_local()
            }
            Some(NavTarget::Dashboard) => {
                self.go(Page::Dashboard);
                self.dashboard.confirm = None;
                self.dashboard.message = None;
                self.refresh_dashboard()
            }
            None => Task::none(),
        };
        self.with_snap(task)
    }

    fn on_window_resize(&mut self, _id: cosmic::iced::window::Id, width: f32, _height: f32) {
        self.window_width = width;
    }

    fn on_escape(&mut self) -> Task<Self::Message> {
        // In the player: out of full screen first, then out of the player.
        if self.playing_inside() {
            if self.player_ui.fullscreen {
                return self.update(Message::Player(PlayerMsg::ToggleFullscreen));
            }
            return self.update(Message::Player(PlayerMsg::Stop));
        }
        if self.core.window.show_context {
            self.core.window.show_context = false;
            return Task::none();
        }
        self.update(Message::Back)
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        if self.back.is_empty() {
            return Vec::new();
        }
        vec![tip(widget::button::icon(icon::from_name("go-previous-symbolic")).on_press(Message::Back), "Back (Esc)")]
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let mut els = Vec::new();
        if matches!(self.page, Page::Home) && self.account.is_some() {
            els.push(tip(
                widget::button::icon(icon::from_name("view-refresh-symbolic")).on_press(Message::RefreshHome),
                "Refresh",
            ));
        }
        if matches!(self.page, Page::Dashboard) {
            els.push(tip(
                widget::button::icon(icon::from_name("view-refresh-symbolic")).on_press(Message::Dashboard(DashMsg::Refresh)),
                "Refresh",
            ));
        }
        if self.account.is_some() {
            let button = widget::button::icon(icon::from_name("system-users-symbolic"))
                .selected(self.sync_active())
                .on_press(Message::Sync(SyncMsg::ToggleMenu));
            els.push(tip(button, "SyncPlay"));
        }
        els.push(tip(
            widget::button::icon(icon::from_name("document-open-symbolic")).on_press(Message::OpenFile),
            "Play a file",
        ));
        els.push(tip(
            widget::button::icon(icon::from_name("emblem-system-symbolic")).on_press(Message::ToggleSettings),
            "Settings",
        ));
        els
    }

    fn context_drawer(&self) -> Option<cosmic::app::context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }
        if self.sync.menu {
            return Some(
                cosmic::app::context_drawer::context_drawer(self.view_syncplay(), Message::Sync(SyncMsg::ToggleMenu))
                    .title("SyncPlay"),
            );
        }
        Some(
            cosmic::app::context_drawer::context_drawer(self.view_settings(), Message::ToggleSettings)
                .title("Settings"),
        )
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let mut subs = Vec::new();
        if self.startup_file.is_some() {
            subs.push(cosmic::iced::event::listen_with(|event, _, _| match event {
                cosmic::iced::Event::Window(cosmic::iced::window::Event::Opened { .. }) => Some(Message::WindowOpened),
                _ => None,
            }));
        }
        #[cfg(target_os = "linux")]
        subs.push(crate::theme::accent_changes().map(Message::DesktopAccent));
        if self.playing_inside() {
            subs.push(Subscription::run(player::frames));
            subs.push(cosmic::iced::keyboard::listen().map(player::key_message));
            subs.push(time::every(Duration::from_millis(250)).map(|_| Message::PlayerTick));
        } else if self.player.is_some() {
            subs.push(time::every(Duration::from_millis(500)).map(|_| Message::PlayerTick));
        }
        if matches!(self.page, Page::Home) && self.home.featured.len() > 1 {
            subs.push(time::every(Duration::from_secs(9)).map(|_| Message::FeaturedTick));
        }
        // The session's WebSocket, for SyncPlay, while signed in.
        if let Some(account) = &self.account {
            let key = (account.url.clone(), account.token.clone());
            subs.push(Subscription::run_with(key, |(url, token)| {
                use cosmic::iced::futures::StreamExt;
                crate::syncplay::socket(url, token).map(|e| Message::Sync(SyncMsg::Socket(e)))
            }));
        }
        // The dashboard keeps up, faster while a task runs.
        if matches!(self.page, Page::Dashboard) {
            let every = if self.dashboard.busy() { Duration::from_secs(2) } else { Duration::from_secs(5) };
            subs.push(time::every(every).map(|_| Message::Dashboard(DashMsg::Refresh)));
        }
        if !self.sync.notices.is_empty() {
            subs.push(time::every(Duration::from_millis(500)).map(|_| Message::Sync(SyncMsg::NoticeTick)));
        }
        if self.signin.quick_connect.is_some() && matches!(self.page, Page::SignIn) {
            subs.push(time::every(Duration::from_secs(2)).map(|_| Message::QuickConnectPoll));
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        let task = self.handle(message);
        let task = self.with_snap(task);
        #[cfg(windows)]
        let task = Task::batch([task, self.sync_frame()]);
        task
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if self.playing_inside() {
            return self.view_player();
        }
        let body = match &self.page {
            Page::SignIn => self.view_signin(),
            Page::Home => self.view_home(),
            Page::Library(lib) => self.view_library(lib),
            Page::Details(d) => self.view_details(d),
            Page::Search => self.view_search(),
            Page::Local => self.view_local(),
            Page::Dashboard => self.view_dashboard(),
        };
        let mut col = widget::column::with_capacity(2);
        if let Some(err) = &self.error {
            col = col.push(widgets::error_banner(err));
        }
        let col = col.push(body).width(Length::Fill).height(Length::Fill);
        // A click anywhere on the page closes the sidebar, as well as
        // doing what it does.
        let close_nav = (self.core.nav_bar_active() && self.nav_model().is_some()).then_some(Message::CloseNav);
        // Scroll reports rebuild the page: the library grid needs one every
        // SCROLL_BUCKET px to build the rows coming into view (it keeps two
        // spare rows, far more), other pages only where scrolling stopped.
        let step = if matches!(self.page, Page::Library(_)) { SCROLL_BUCKET } else { f32::INFINITY };
        let gestures = crate::input::gestures(col).on_press_maybe(close_nav).throttle(move |m| match m {
            Message::Scrolled(_, px) => Some((px / step).floor() as i32),
            _ => None,
        });
        let page = widget::container(gestures)
            .class(cosmic::theme::Container::custom(crate::theme::page))
            .width(Length::Fill)
            .height(Length::Fill);
        match self.sync_notices() {
            Some(notices) => cosmic::iced::widget::stack![page, notices].into(),
            None => page.into(),
        }
    }

    /// COSMIC's own theme settings changed (its accent among them).
    fn system_theme_update(&mut self, _keys: &[&'static str], new_theme: &cosmic::cosmic_theme::Theme) -> Task<Self::Message> {
        let a = new_theme.accent_color();
        self.update(Message::DesktopAccent(Srgb::new(a.red, a.green, a.blue)))
    }
}

impl App {
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowOpened => {
                if let Some(path) = self.startup_file.take() {
                    // A moment more for the window's first frame.
                    return cosmic::task::future(async move {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        Message::PlayLocal(path)
                    });
                }
            }
            Message::RestoreScroll(y) => self.scroll_to(y),
            Message::Scrolled(y, pixels) => {
                self.scroll_y = y;
                if let Page::Library(lib) = &mut self.page {
                    lib.offset = pixels;
                }
            }
            // ── Sign-in ──────────────────────────────────────────────────────
            Message::ServerInput(s) => self.signin.server = s,
            Message::Connect => {
                self.signin.busy = true;
                self.signin.error = None;
                self.signin.found = None;
                self.signin.quick_connect = None;
                let input = self.signin.server.clone();
                return cosmic::task::future(async move {
                    let result = match jellyfin::find_server(&input).await {
                        Ok((url, info)) => {
                            let qc = jellyfin::quick_connect_enabled(&url).await;
                            Ok((url, info.server_name, qc))
                        }
                        Err(e) => Err(e),
                    };
                    Message::ServerFound(result)
                });
            }
            Message::ServerFound(result) => match result {
                Ok((url, name, quick)) => {
                    self.signin.found = Some((url.clone(), name));
                    if quick {
                        return cosmic::task::future(async move {
                            Message::QuickConnectStarted(jellyfin::quick_connect_start(&url).await)
                        });
                    }
                    self.signin.busy = false;
                }
                Err(e) => {
                    self.signin.busy = false;
                    self.signin.error = Some(e);
                }
            },
            Message::QuickConnectStarted(result) => {
                self.signin.busy = false;
                match result {
                    Ok(qc) => self.signin.quick_connect = Some(qc),
                    Err(e) => log::info!("Quick Connect unavailable: {e}"),
                }
            }
            Message::QuickConnectPoll => {
                if let (Some((url, _)), Some(qc)) = (&self.signin.found, &self.signin.quick_connect) {
                    let (url, secret) = (url.clone(), qc.secret.clone());
                    return cosmic::task::future(async move {
                        Message::QuickConnectChecked(jellyfin::quick_connect_approved(&url, &secret).await)
                    });
                }
            }
            Message::QuickConnectChecked(Ok(true)) => {
                if let (Some((url, name)), Some(qc)) = (self.signin.found.clone(), self.signin.quick_connect.take()) {
                    self.signin.busy = true;
                    return cosmic::task::future(async move {
                        Message::SignedIn(jellyfin::quick_connect_finish(&url, &name, &qc.secret).await)
                    });
                }
            }
            Message::QuickConnectChecked(Ok(false)) => {}
            Message::QuickConnectChecked(Err(e)) => {
                // The code expired or the server went away: start over.
                self.signin.quick_connect = None;
                self.signin.error = Some(format!("Quick Connect stopped: {e}"));
            }
            Message::UserInput(s) => self.signin.user = s,
            Message::PasswordInput(s) => self.signin.password = s,
            Message::PasswordSignIn => {
                if let Some((url, name)) = self.signin.found.clone() {
                    self.signin.busy = true;
                    self.signin.error = None;
                    let user = self.signin.user.clone();
                    let password = std::mem::take(&mut self.signin.password);
                    return cosmic::task::future(async move {
                        Message::SignedIn(jellyfin::sign_in_with_password(&url, &name, &user, &password).await)
                    });
                }
            }
            Message::SignedIn(result) => {
                self.signin.busy = false;
                match result {
                    Ok(account) => {
                        self.accounts.upsert(account.clone());
                        return self.use_account(Some(account));
                    }
                    Err(e) => self.signin.error = Some(e),
                }
            }
            Message::AddAccount => {
                self.core.window.show_context = false;
                self.signin = SignIn::default();
                self.push(Page::SignIn);
            }
            Message::SwitchAccount(key) => {
                let account = self.accounts.find(&key).cloned();
                self.core.window.show_context = false;
                return self.use_account(account);
            }
            Message::SignOut => {
                if let Some(account) = self.account.take() {
                    self.accounts.remove(&account.key());
                    let next = self.accounts.accounts.first().cloned();
                    let signed_out = cosmic::task::future(async move {
                        jellyfin::sign_out(&account).await;
                        Message::Cancelled
                    });
                    self.core.window.show_context = false;
                    return Task::batch([signed_out, self.use_account(next)]);
                }
            }

            // ── Browsing ─────────────────────────────────────────────────────
            Message::RefreshHome => return self.refresh_home(),
            Message::HomeLoaded(key, result) => {
                if self.account.as_ref().map(|a| a.key()) != Some(key) {
                    return Task::none();
                }
                self.home.loading = false;
                match result {
                    Ok(data) => {
                        let libraries_changed = self.home.libraries.iter().map(|l| &l.id).ne(data.libraries.iter().map(|l| &l.id));
                        let mut keys: Vec<Key> = Vec::new();
                        keys.extend(data.featured.iter().filter_map(|i| i.backdrop()).map(|b| Self::key(b, BACKDROP_W)));
                        keys.extend(data.featured.iter().filter_map(|i| i.logo()).map(|b| Self::key(b, LOGO_W)));
                        keys.extend(Self::landscape_keys(&data.libraries));
                        keys.extend(Self::landscape_keys(&data.resume));
                        keys.extend(Self::landscape_keys(&data.next_up));
                        for (_, items) in &data.latest {
                            keys.extend(Self::poster_keys(items));
                        }
                        let keep_featured = self.home.featured.len() == data.featured.len() && self.home.loaded;
                        let featured_at = if keep_featured { self.home.featured_at } else { 0 };
                        self.home = Home {
                            libraries: data.libraries,
                            resume: data.resume,
                            next_up: data.next_up,
                            latest: data.latest,
                            // A refresh keeps the banner where it is.
                            featured: if keep_featured { std::mem::take(&mut self.home.featured) } else { data.featured },
                            featured_at,
                            loaded: true,
                            loading: false,
                        };
                        if libraries_changed {
                            self.rebuild_nav();
                        }
                        return self.fetch_images(keys);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::FeaturedTick => {
                if !self.home.featured.is_empty() {
                    self.home.featured_at = (self.home.featured_at + 1) % self.home.featured.len();
                }
            }
            Message::FeaturedStep(step) => {
                let n = self.home.featured.len() as i32;
                if n > 0 {
                    self.home.featured_at = ((self.home.featured_at as i32 + step).rem_euclid(n)) as usize;
                }
            }
            Message::FeaturedPick(i) => {
                if i < self.home.featured.len() {
                    self.home.featured_at = i;
                }
            }
            Message::ImageLoaded(key, result) => {
                self.images_pending.remove(&key);
                match result {
                    Ok(h) => {
                        self.images.insert(key, h);
                    }
                    Err(e) => log::debug!("image {:?}: {e}", key.img),
                }
            }
            Message::Open(item) => return self.open_item(item),
            Message::Library(msg) => return self.update_library(msg),
            Message::DetailsLoaded(id, result) => {
                let Page::Details(d) = &mut self.page else { return Task::none() };
                if d.item.id != id {
                    return Task::none();
                }
                d.loading = false;
                match result {
                    Ok(data) => {
                        let data = *data;
                        let mut keys = Vec::new();
                        if let Some(b) = data.item.backdrop() {
                            keys.push(Self::key(b, BACKDROP_W));
                        }
                        if let Some(l) = data.item.logo() {
                            keys.push(Self::key(l, LOGO_W));
                        }
                        keys.extend(Self::poster_keys([&data.item]));
                        if data.item.is_episode() {
                            keys.extend(Self::landscape_keys([&data.item]));
                        }
                        keys.extend(Self::poster_keys(&data.seasons));
                        keys.extend(Self::poster_keys(&data.similar));
                        keys.extend(Self::landscape_keys(&data.episodes));
                        keys.extend(data.item.people.iter().filter_map(|p| p.image()).map(|i| Self::key(i, PERSON_W)));
                        d.item = data.item;
                        d.seasons = data.seasons;
                        d.season = data.season;
                        d.episodes = data.episodes;
                        d.next_up = data.next_up;
                        d.similar = data.similar;
                        return self.fetch_images(keys);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::SeasonPick(season_id) => {
                let Some(account) = self.account.clone() else { return Task::none() };
                if let Page::Details(d) = &mut self.page {
                    if d.season.as_deref() == Some(&season_id) {
                        return Task::none();
                    }
                    d.season = Some(season_id.clone());
                    d.episodes.clear();
                    d.episodes_loading = true;
                    let series = d.item.id.clone();
                    return cosmic::task::future(async move {
                        let result = jellyfin::episodes(&account, &series, &season_id).await;
                        Message::EpisodesLoaded(season_id, result)
                    });
                }
            }
            Message::EpisodesLoaded(season_id, result) => {
                let Page::Details(d) = &mut self.page else { return Task::none() };
                if d.season.as_deref() != Some(&season_id) {
                    return Task::none();
                }
                d.episodes_loading = false;
                match result {
                    Ok(eps) => {
                        let keys = Self::landscape_keys(&eps);
                        d.episodes = eps;
                        return self.fetch_images(keys);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::TogglePlayed(item) => {
                let Some(account) = self.account.clone() else { return Task::none() };
                let played = !item.played();
                return cosmic::task::future(async move {
                    Message::UserDataSaved(jellyfin::set_played(&account, &item.id, played).await)
                });
            }
            Message::ToggleFavorite(item) => {
                let Some(account) = self.account.clone() else { return Task::none() };
                let favorite = !item.favorite();
                return cosmic::task::future(async move {
                    Message::UserDataSaved(jellyfin::set_favorite(&account, &item.id, favorite).await)
                });
            }
            Message::UserDataSaved(result) => {
                if let Err(e) = result {
                    self.error = Some(format!("Couldn't save that to the server: {e}"));
                }
                return self.refresh_details();
            }
            Message::Back => {
                if let Some((page, y)) = self.back.pop() {
                    self.page = page;
                    self.scroll_to(y);
                    if matches!(self.page, Page::Home) {
                        return self.refresh_home();
                    }
                    return self.refresh_details();
                }
                if matches!(self.page, Page::SignIn) && self.account.is_some() {
                    self.page = Page::Home;
                }
            }
            Message::SearchInput(s) => self.search.term = s,
            Message::SearchSubmit => {
                let Some(account) = self.account.clone() else { return Task::none() };
                let term = self.search.term.trim().to_string();
                if term.is_empty() {
                    return Task::none();
                }
                self.search.loading = true;
                return cosmic::task::future(async move {
                    let result = jellyfin::search(&account, &term, 60).await;
                    Message::SearchResults(term, result)
                });
            }
            Message::SearchResults(term, result) => {
                if term != self.search.term.trim() {
                    return Task::none();
                }
                self.search.loading = false;
                match result {
                    Ok(items) => {
                        let mut keys = Self::poster_keys(items.iter().filter(|i| !i.is_episode()));
                        keys.extend(Self::landscape_keys(items.iter().filter(|i| i.is_episode())));
                        self.search.searched = term;
                        self.search.results = items;
                        return self.fetch_images(keys);
                    }
                    Err(e) => self.error = Some(e),
                }
            }

            // ── Playback ─────────────────────────────────────────────────────
            Message::Play { item, resume } => {
                // In a SyncPlay group, playing something sets the group's queue.
                if self.sync_active() {
                    return self.group_play(item, resume);
                }
                return self.prepare_play(item, resume, None, None);
            }
            Message::PlaybackReady(result) => match result {
                // A SyncPlay item the group has moved on from.
                Ok(start) if start.sync_pid.is_some() && !self.sync_wants(start.sync_pid.as_deref()) => {}
                Ok(start) => return self.start_remote(*start),
                Err(e) => {
                    self.error = Some(e);
                    return self.sync_load_failed();
                }
            },
            Message::PlayerTick => return self.player_tick(),
            Message::Sync(msg) => return self.update_sync(msg),
            Message::Dashboard(msg) => return self.update_dashboard(msg),
            Message::AdminChecked(key, result) => {
                if self.account.as_ref().map(|a| a.key()) == Some(key) {
                    match result {
                        Ok(admin) => self.admin = admin,
                        Err(e) => log::info!("couldn't tell if this user is an administrator: {e}"),
                    }
                    self.rebuild_nav();
                }
            }
            Message::Player(msg) => return self.update_player(msg),
            Message::NextEpisode(result) => match result {
                Ok(Some(next)) => return self.update(Message::Play { item: next, resume: false }),
                Ok(None) | Err(_) => {
                    if let Some(p) = &self.player {
                        p.quit();
                    }
                }
            },
            Message::Reported(result) => {
                if let Err(e) = result {
                    log::warn!("playback report: {e}");
                }
            }

            // ── Local files ──────────────────────────────────────────────────
            Message::OpenFile => {
                return cosmic::task::future(async move {
                    let dialog = file_chooser::open::Dialog::new().title("Play a File");
                    match dialog.open_file().await {
                        Ok(r) => match r.url().to_file_path() {
                            Ok(p) => Message::FilePicked(p),
                            Err(()) => Message::Cancelled,
                        },
                        Err(file_chooser::Error::Cancelled) => Message::Cancelled,
                        Err(e) => Message::Reported(Err(e.to_string())),
                    }
                });
            }
            Message::FilePicked(path) | Message::PlayLocal(path) => return self.play_local(path),
            Message::AddFolder => {
                return cosmic::task::future(async move {
                    let dialog = file_chooser::open::Dialog::new().title("Add a Video Folder");
                    match dialog.open_folder().await {
                        Ok(r) => match r.url().to_file_path() {
                            Ok(p) => Message::FolderPicked(p),
                            Err(()) => Message::Cancelled,
                        },
                        Err(file_chooser::Error::Cancelled) => Message::Cancelled,
                        Err(e) => Message::Reported(Err(e.to_string())),
                    }
                });
            }
            Message::FolderPicked(path) => {
                if !self.settings.local_folders.contains(&path) {
                    self.settings.local_folders.push(path);
                    self.settings.save();
                }
                return self.scan_local();
            }
            Message::RemoveFolder(i) => {
                if i < self.settings.local_folders.len() {
                    self.settings.local_folders.remove(i);
                    self.settings.save();
                }
                return self.scan_local();
            }
            Message::LocalScanned(found) => {
                self.local_scanning = false;
                self.local = found;
            }

            // ── Settings ─────────────────────────────────────────────────────
            Message::ToggleSettings => {
                // From the SyncPlay menu, straight over to Settings.
                self.core.window.show_context = !self.core.window.show_context || self.sync.menu;
                self.sync.menu = false;
            }
            Message::SetTheme(i) => {
                let choice = if i == 1 { ThemeChoice::System } else { ThemeChoice::Jellyfin };
                if choice != self.settings.theme {
                    self.settings.theme = choice;
                    self.settings.save();
                    return self.apply_theme();
                }
            }
            Message::SetAccent(i) => {
                let choice = AccentChoice::ALL.get(i).copied().unwrap_or_default();
                if choice != self.settings.accent {
                    self.settings.accent = choice;
                    self.settings.save();
                    return self.apply_theme();
                }
            }
            Message::DesktopAccent(c) => {
                let close = |a: Srgb, b: Srgb| (a.red - b.red).abs() + (a.green - b.green).abs() + (a.blue - b.blue).abs() < 0.01;
                if self.desktop_accent.is_some_and(|old| close(old, c)) {
                    return Task::none();
                }
                self.desktop_accent = Some(c);
                if self.settings.theme == ThemeChoice::Jellyfin && self.settings.accent == AccentChoice::Desktop {
                    return self.apply_theme();
                }
            }
            Message::CloseNav => self.core.nav_bar_set_toggled(false),
            Message::SetFullscreen(on) => {
                self.settings.fullscreen = on;
                self.settings.save();
            }
            Message::SetSeparateWindow(on) => {
                self.settings.separate_window = on;
                self.settings.save();
            }
            Message::SetHwdec(on) => {
                self.settings.hwdec = on;
                self.settings.save();
            }
            Message::SetSkip(i) => {
                self.settings.skip = SkipMode::ALL.get(i).copied().unwrap_or_default();
                self.settings.save();
            }
            Message::SetAutoplay(on) => {
                self.settings.autoplay_next = on;
                self.settings.save();
            }
            Message::OpenMpvConfig => {
                let dir = crate::platform::mpv_config_dir();
                let _ = std::fs::create_dir_all(&dir);
                crate::platform::open(&dir.to_string_lossy());
            }

            Message::DismissError => self.error = None,
            Message::Cancelled => {}
        }
        Task::none()
    }

}

impl Drop for App {
    fn drop(&mut self) {
        // Quitting with something playing: tell the server where it stopped.
        if let Some(pb) = self.playback.take() {
            if let (Some(remote), true) = (pb.remote, pb.started) {
                jellyfin::report_stopped_now(&remote.account, &remote.report(pb.position, pb.paused));
            }
        }
    }
}
