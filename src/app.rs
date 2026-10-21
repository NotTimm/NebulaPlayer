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
use crate::jellyfin::{self, Account, Accounts, ImageRef, Item, Report, SortBy};
use crate::mpv;
use crate::settings::{Settings, ThemeChoice};

mod details;
mod home;
mod library;
mod local;
mod search;
mod settings_view;
mod signin;
mod widgets;

pub use local::LocalFile;

/// Image widths fetched for each use (about twice the size shown, for
/// HiDPI screens).
pub const POSTER_W: u32 = 320;
pub const LANDSCAPE_W: u32 = 560;
pub const BACKDROP_W: u32 = 1920;
pub const LOGO_W: u32 = 700;
pub const PERSON_W: u32 = 200;

/// Items fetched per library page.
const PAGE_SIZE: usize = 100;

/// Where the sidebar can take the user.
#[derive(Clone, Debug)]
enum NavTarget {
    Home,
    Library(Item),
    Search,
    Local,
}

pub enum Page {
    Home,
    Library(Box<LibraryPage>),
    Details(Box<DetailsPage>),
    Search,
    Local,
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

pub struct LibraryPage {
    pub parent: Item,
    pub items: Vec<Item>,
    pub total: usize,
    pub sort: SortBy,
    pub loading: bool,
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
    /// `mpv::State::loaded` when this started, to tell its load from the
    /// previous file's.
    loaded_before: u64,
    started: bool,
    finished: bool,
    position: f64,
    paused: bool,
    last_report: Instant,
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
    info: jellyfin::PlaybackInfo,
    resume: bool,
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
    LibraryLoaded(String, usize, Result<(Vec<Item>, usize), String>),
    LibraryMore,
    LibrarySort(usize),
    DetailsLoaded(String, Result<Box<DetailsData>, String>),
    SeasonPick(String),
    EpisodesLoaded(String, Result<Vec<Item>, String>),
    TogglePlayed(Item),
    ToggleFavorite(Item),
    UserDataSaved(Result<(), String>),
    Back,
    Scrolled(f32),
    WindowOpened,
    SearchInput(String),
    SearchSubmit,
    SearchResults(String, Result<Vec<Item>, String>),

    // Playback
    Play { item: Item, resume: bool },
    PlaybackReady(Result<Box<PlayStart>, String>),
    PlayerTick,
    NextEpisode(Result<Option<Item>, String>),
    Reported(Result<(), String>),

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
    SetFullscreen(bool),
    SetHwdec(bool),
    SetAutoplay(bool),
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
    images: HashMap<Key, images::Picture>,
    images_pending: HashSet<Key>,
    player: Option<mpv::Player>,
    playback: Option<Playback>,
    /// A file from the command line, played once the window is up: mpv
    /// setting up its renderer while the app sets up its own has crashed
    /// graphics drivers.
    startup_file: Option<PathBuf>,
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
            (NavTarget::Library(lib), Page::Library(p)) => lib.id == p.parent.id,
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

    fn open_library(&mut self, parent: Item, from_nav: bool) -> Task<Message> {
        let page = Page::Library(Box::new(LibraryPage {
            parent: parent.clone(),
            items: Vec::new(),
            total: 0,
            sort: SortBy::Name,
            loading: true,
        }));
        if from_nav {
            self.go(page);
        } else {
            self.push(page);
        }
        self.load_library_page(parent, SortBy::Name, 0)
    }

    fn load_library_page(&self, parent: Item, sort: SortBy, start: usize) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        cosmic::task::future(async move {
            let result = jellyfin::library_page(&account, &parent, sort, start, PAGE_SIZE).await;
            Message::LibraryLoaded(parent.id, start, result)
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
        self.account = account;
        self.home = Home::default();
        self.back.clear();
        self.scroll_to(0.0);
        self.images.clear();
        self.images_pending.clear();
        self.rebuild_nav();
        if self.account.is_some() {
            self.page = Page::Home;
            self.refresh_home()
        } else {
            self.signin = SignIn::default();
            self.page = Page::SignIn;
            Task::none()
        }
    }

    // ── Playback ─────────────────────────────────────────────────────────────

    fn player_options(&self) -> mpv::Options {
        mpv::Options {
            fullscreen: self.settings.fullscreen,
            hwdec: self.settings.hwdec,
            config_dir: crate::platform::mpv_config_dir(),
            watch_later_dir: crate::platform::data_dir().join("watch_later"),
        }
    }

    /// The running player, or a new one if mpv's window was closed.
    fn ensure_player(&mut self) -> Result<&mpv::Player, String> {
        if self.player.as_ref().is_some_and(|p| p.state().closed) {
            self.player = None;
        }
        if self.player.is_none() {
            self.player = Some(mpv::Player::new(&self.player_options())?);
        }
        Ok(self.player.as_ref().unwrap())
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
            start: if start.resume { Some(start.item.resume_seconds()) } else { None },
            headers: account.stream_headers(),
            audio,
            subtitle,
            external_subs: source.external_subs(&account, &start.item.id),
            remember_position: false,
        };
        let loaded_before = match self.ensure_player() {
            Ok(p) => {
                let before = p.state().loaded;
                if let Err(e) = p.load(&load) {
                    self.error = Some(e);
                    return stopped;
                }
                before
            }
            Err(e) => {
                self.error = Some(e);
                return stopped;
            }
        };
        self.playback = Some(Playback {
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
        });
        stopped
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
            Ok(p) => {
                let loaded_before = p.state().loaded;
                if let Err(e) = p.load(&load) {
                    self.error = Some(e);
                    return stopped;
                }
                self.playback = Some(Playback {
                    remote: None,
                    loaded_before,
                    started: false,
                    finished: false,
                    position: 0.0,
                    paused: false,
                    last_report: Instant::now(),
                });
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
        if state.closed {
            self.player = None;
            let stopped = self.end_playback();
            // Continue Watching and the page's progress bars have changed.
            return Task::batch([stopped, self.refresh_home(), self.refresh_details()]);
        }
        let Some(pb) = &mut self.playback else { return Task::none() };
        if state.loaded > pb.loaded_before && state.ended.is_none() && state.duration > 0.0 {
            pb.position = state.position;
        }
        let paused_changed = pb.paused != state.paused;
        pb.paused = state.paused;

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
                        state.error.unwrap_or_else(|| "unknown error".into())
                    ));
                    tasks.push(self.end_playback());
                    if let Some(p) = &self.player {
                        p.quit();
                    }
                }
                _ => {}
            }
        }
        Task::batch(tasks)
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
            images: HashMap::new(),
            images_pending: HashSet::new(),
            player: None,
            playback: None,
            startup_file: flags,
            error: None,
        };
        app.set_header_title("Nebula Player".into());
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
            _ => Some(&self.nav),
        }
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Self::Message> {
        self.nav.activate(id);
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
            None => Task::none(),
        };
        self.with_snap(task)
    }

    fn on_escape(&mut self) -> Task<Self::Message> {
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
        if self.player.is_some() {
            subs.push(time::every(Duration::from_millis(500)).map(|_| Message::PlayerTick));
        }
        if matches!(self.page, Page::Home) && self.home.featured.len() > 1 {
            subs.push(time::every(Duration::from_secs(9)).map(|_| Message::FeaturedTick));
        }
        if self.signin.quick_connect.is_some() && matches!(self.page, Page::SignIn) {
            subs.push(time::every(Duration::from_secs(2)).map(|_| Message::QuickConnectPoll));
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        let task = self.handle(message);
        self.with_snap(task)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let body = match &self.page {
            Page::SignIn => self.view_signin(),
            Page::Home => self.view_home(),
            Page::Library(lib) => self.view_library(lib),
            Page::Details(d) => self.view_details(d),
            Page::Search => self.view_search(),
            Page::Local => self.view_local(),
        };
        let mut col = widget::column::with_capacity(2);
        if let Some(err) = &self.error {
            col = col.push(widgets::error_banner(err));
        }
        col.push(body).width(Length::Fill).height(Length::Fill).into()
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
            Message::Scrolled(y) => {
                self.scroll_y = y;
                // Near the bottom of a library, its next page loads.
                if y > 0.85 && matches!(self.page, Page::Library(_)) {
                    return self.handle(Message::LibraryMore);
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
            Message::LibraryLoaded(id, start, result) => {
                let Page::Library(lib) = &mut self.page else { return Task::none() };
                if lib.parent.id != id || lib.items.len() != start {
                    return Task::none();
                }
                lib.loading = false;
                match result {
                    Ok((items, total)) => {
                        let keys = Self::poster_keys(&items);
                        lib.items.extend(items);
                        lib.total = total;
                        return self.fetch_images(keys);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::LibraryMore => {
                if let Page::Library(lib) = &mut self.page {
                    if !lib.loading && lib.items.len() < lib.total {
                        lib.loading = true;
                        let (parent, sort, start) = (lib.parent.clone(), lib.sort, lib.items.len());
                        return self.load_library_page(parent, sort, start);
                    }
                }
            }
            Message::LibrarySort(i) => {
                if let Page::Library(lib) = &mut self.page {
                    let sort = SortBy::ALL.get(i).copied().unwrap_or_default();
                    if sort != lib.sort {
                        lib.sort = sort;
                        lib.items.clear();
                        lib.total = 0;
                        lib.loading = true;
                        let parent = lib.parent.clone();
                        return self.load_library_page(parent, sort, 0);
                    }
                }
            }
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
                let Some(account) = self.account.clone() else { return Task::none() };
                return cosmic::task::future(async move {
                    let result = async {
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
                        let info = jellyfin::playback_info(&account, &item.id).await?;
                        Ok(Box::new(PlayStart { item, info, resume }))
                    }
                    .await;
                    Message::PlaybackReady(result)
                });
            }
            Message::PlaybackReady(result) => match result {
                Ok(start) => return self.start_remote(*start),
                Err(e) => self.error = Some(e),
            },
            Message::PlayerTick => return self.player_tick(),
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
                self.core.window.show_context = !self.core.window.show_context;
            }
            Message::SetTheme(i) => {
                let choice = if i == 1 { ThemeChoice::System } else { ThemeChoice::Jellyfin };
                if choice != self.settings.theme {
                    self.settings.theme = choice;
                    self.settings.save();
                    return cosmic::command::set_theme(crate::theme::build(choice));
                }
            }
            Message::SetFullscreen(on) => {
                self.settings.fullscreen = on;
                self.settings.save();
            }
            Message::SetHwdec(on) => {
                self.settings.hwdec = on;
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
