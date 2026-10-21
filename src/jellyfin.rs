//! Jellyfin: sign-in, the home screen's sections, browsing, images and
//! playback reports.
//!
//! Sign-in uses Quick Connect where the server allows it: the app shows a
//! code, the user approves it in Jellyfin, and the app gets its own access
//! token, so no password passes through here. Servers with Quick Connect
//! off fall back to username and password, used once and never saved. Only
//! tokens are stored (`jellyfin.json` in the config folder, owner-only);
//! each one shows up in Jellyfin as a device and can be revoked there.
//!
//! Playback is reported the way Jellyfin's own clients do it
//! (`/Sessions/Playing`, `/Progress` every few seconds, `/Stopped` with the
//! last position), so the server keeps resume points, marks things watched
//! past 90 % and shows the session on its dashboard.
//!
//! Routes: Jellyfin 10.9 moved the per-user `/Users/{id}/...` routes to
//! `/UserViews`, `/UserItems/...` and `/Items/...?userId=`, and marked the
//! old ones for removal. The new route is asked first; a server that
//! answers 404 (10.8 and older) is asked the old one ([`UserRoute`]).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const CLIENT: &str = "Nebula Player";

/// Jellyfin counts time in ticks of 100 ns.
pub const TICKS_PER_SECOND: f64 = 10_000_000.0;

/// One signed-in user on one server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Base URL without a trailing slash, e.g. `https://jellyfin.example:8920`.
    pub url: String,
    pub server_id: String,
    pub server_name: String,
    pub user_id: String,
    pub user_name: String,
    pub token: String,
}

impl Account {
    /// Stable id for this server + user.
    pub fn key(&self) -> String {
        format!("{}-{}", self.server_id, self.user_id)
    }

    pub fn label(&self) -> String {
        format!("{} on {}", self.user_name, self.server_name)
    }

    pub fn auth(&self) -> String {
        auth_header(Some(&self.token))
    }

    fn user(&self) -> String {
        format!("{}/Users/{}", self.url, self.user_id)
    }

    /// `/Items/...` for one user (Jellyfin 10.9+) and the route it
    /// replaced.
    fn items_route(&self, rest: &str, params: &str) -> UserRoute {
        UserRoute {
            current: query(self, &format!("/Items{rest}"), &format!("userId={}&{params}", self.user_id)),
            legacy: query(self, &format!("/Users/{}/Items{rest}", self.user_id), params),
        }
    }

    /// An image of an item, at most `max_width` wide.
    pub fn image_url(&self, img: &ImageRef, max_width: u32) -> String {
        let mut url = format!(
            "{}/Items/{}/Images/{}?maxWidth={max_width}&quality=90",
            self.url,
            img.item_id,
            img.kind.as_str()
        );
        if let Some(tag) = &img.tag {
            url.push_str("&tag=");
            url.push_str(tag);
        }
        url
    }

    /// The file itself, as stored on the server: mpv plays (nearly)
    /// anything, so nothing is transcoded.
    pub fn stream_url(&self, item_id: &str, source: &MediaSource, play_session_id: &str) -> String {
        format!(
            "{}/Videos/{item_id}/stream?static=true&mediaSourceId={}&playSessionId={play_session_id}",
            self.url, source.id
        )
    }

    /// Sent with every request mpv makes for a stream (and its subtitles).
    /// The token header has no commas, which mpv's header list splits on.
    pub fn stream_headers(&self) -> Vec<String> {
        vec![format!("X-Emby-Token: {}", self.token)]
    }
}

// ── Stored accounts ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Accounts {
    /// Identifies this install to Jellyfin (one device entry per install).
    pub device_id: String,
    pub accounts: Vec<Account>,
}

fn accounts_path() -> std::path::PathBuf {
    crate::platform::config_dir().join("jellyfin.json")
}

impl Accounts {
    pub fn load() -> Self {
        let mut a: Accounts = std::fs::read(accounts_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if a.device_id.is_empty() {
            a.device_id = new_device_id();
            a.save();
        }
        a
    }

    pub fn save(&self) {
        let path = accounts_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(&path, json);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    pub fn find(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.key() == key)
    }

    /// Adds or replaces (same server + user) an account.
    pub fn upsert(&mut self, account: Account) {
        self.accounts.retain(|a| a.key() != account.key());
        self.accounts.push(account);
        self.save();
    }

    pub fn remove(&mut self, key: &str) {
        self.accounts.retain(|a| a.key() != key);
        self.save();
    }
}

fn new_device_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    std::process::id().hash(&mut h);
    device_name().hash(&mut h);
    format!("nebula-player-{:016x}", h.finish())
}

fn device_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Desktop".into())
}

static DEVICE_ID: OnceLock<String> = OnceLock::new();

fn auth_header(token: Option<&str>) -> String {
    let device_id = DEVICE_ID.get_or_init(|| Accounts::load().device_id);
    let mut h = format!(
        r#"MediaBrowser Client="{CLIENT}", Device="{}", DeviceId="{device_id}", Version="{}""#,
        device_name().replace('"', ""),
        env!("CARGO_PKG_VERSION")
    );
    if let Some(t) = token {
        h.push_str(&format!(r#", Token="{t}""#));
    }
    h
}

// ── HTTP ────────────────────────────────────────────────────────────────────

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("http client")
    })
}

/// The addresses to try for what the user typed, dropping a trailing slash
/// or a pasted `/web/...` path. Without a scheme it's https first, then
/// http (home servers like `192.168.1.5:8096` are usually plain http).
pub fn candidate_urls(input: &str) -> Vec<String> {
    let mut url = input.trim().to_string();
    if let Some(i) = url.find("/web") {
        url.truncate(i);
    }
    let url = url.trim_end_matches('/').to_string();
    if url.is_empty() {
        return Vec::new();
    }
    if url.contains("://") {
        vec![url]
    } else {
        vec![format!("https://{url}"), format!("http://{url}")]
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str, auth: &str) -> Result<T, String> {
    let resp = client()
        .get(url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("couldn't reach the server: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("signed out by the server (sign in again)".into());
    }
    if !status.is_success() {
        return Err(format!("server error: HTTP {status}"));
    }
    resp.json().await.map_err(|e| format!("unexpected reply: {e}"))
}

async fn post_json<T: serde::de::DeserializeOwned>(url: &str, auth: &str, body: &serde_json::Value) -> Result<T, String> {
    let resp = client()
        .post(url)
        .header("Authorization", auth)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("couldn't reach the server: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("not authorized".into());
    }
    if !status.is_success() {
        return Err(format!("server error: HTTP {status}"));
    }
    resp.json().await.map_err(|e| format!("unexpected reply: {e}"))
}

/// A per-user request as Jellyfin 10.9+ names it and as older servers do.
struct UserRoute {
    current: String,
    legacy: String,
}

fn not_found(e: &str) -> bool {
    e.contains("HTTP 404")
}

impl UserRoute {
    async fn get<T: serde::de::DeserializeOwned>(&self, account: &Account) -> Result<T, String> {
        match get_json(&self.current, &account.auth()).await {
            Err(e) if not_found(&e) => get_json(&self.legacy, &account.auth()).await,
            r => r,
        }
    }

    /// POST (`on`) or DELETE a flag such as played or favourite.
    async fn set(&self, account: &Account, on: bool) -> Result<(), String> {
        let req = |url: &str| if on { client().post(url) } else { client().delete(url) };
        match send(req(&self.current), &account.auth()).await {
            Err(e) if not_found(&e) => send(req(&self.legacy), &account.auth()).await,
            r => r,
        }
    }
}

/// A request whose reply body doesn't matter (reports, marking played).
async fn send(req: reqwest::RequestBuilder, auth: &str) -> Result<(), String> {
    let resp = req.header("Authorization", auth).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("server error: HTTP {}", resp.status()));
    }
    Ok(())
}

// ── Sign-in ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServerInfo {
    pub server_name: String,
}

/// Finds the Jellyfin server at what the user typed (see [`candidate_urls`]),
/// returning the address that answered.
pub async fn find_server(input: &str) -> Result<(String, ServerInfo), String> {
    let mut last = "Type the address of your Jellyfin server.".to_string();
    for url in candidate_urls(input) {
        match get_json::<ServerInfo>(&format!("{url}/System/Info/Public"), &auth_header(None)).await {
            Ok(info) => return Ok((url, info)),
            Err(e) => last = format!("No Jellyfin server found at {url} ({e})."),
        }
    }
    Err(last)
}

pub async fn quick_connect_enabled(url: &str) -> bool {
    get_json::<bool>(&format!("{url}/QuickConnect/Enabled"), &auth_header(None)).await.unwrap_or(false)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QuickConnect {
    pub secret: String,
    pub code: String,
    #[serde(default)]
    pub authenticated: bool,
}

/// Starts Quick Connect: the returned `code` is what the user approves.
pub async fn quick_connect_start(url: &str) -> Result<QuickConnect, String> {
    post_json(&format!("{url}/QuickConnect/Initiate"), &auth_header(None), &serde_json::json!({})).await
}

/// Whether the user has approved the code yet.
pub async fn quick_connect_approved(url: &str, secret: &str) -> Result<bool, String> {
    let state: QuickConnect =
        get_json(&format!("{url}/QuickConnect/Connect?Secret={secret}"), &auth_header(None)).await?;
    Ok(state.authenticated)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthResult {
    user: AuthUser,
    access_token: String,
    server_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthUser {
    id: String,
    name: String,
}

fn account_from(url: &str, server_name: &str, r: AuthResult) -> Account {
    Account {
        url: url.to_string(),
        server_id: r.server_id,
        server_name: server_name.to_string(),
        user_id: r.user.id,
        user_name: r.user.name,
        token: r.access_token,
    }
}

/// Finishes Quick Connect once approved.
pub async fn quick_connect_finish(url: &str, server_name: &str, secret: &str) -> Result<Account, String> {
    let r = post_json(
        &format!("{url}/Users/AuthenticateWithQuickConnect"),
        &auth_header(None),
        &serde_json::json!({ "Secret": secret }),
    )
    .await?;
    Ok(account_from(url, server_name, r))
}

/// Username/password sign-in, for servers with Quick Connect turned off.
/// The password is sent once to get a token and not kept.
pub async fn sign_in_with_password(url: &str, server_name: &str, user: &str, password: &str) -> Result<Account, String> {
    let r = post_json(
        &format!("{url}/Users/AuthenticateByName"),
        &auth_header(None),
        &serde_json::json!({ "Username": user, "Pw": password }),
    )
    .await
    .map_err(|e| if e == "not authorized" { "wrong username or password".into() } else { e })?;
    Ok(account_from(url, server_name, r))
}

/// Ends this app's session on the server (revokes the token).
pub async fn sign_out(account: &Account) {
    let _ = send(client().post(format!("{}/Sessions/Logout", account.url)), &account.auth()).await;
}

// ── Items ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserData {
    pub played: bool,
    pub playback_position_ticks: i64,
    pub played_percentage: Option<f64>,
    pub unplayed_item_count: Option<u32>,
    pub is_favorite: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Item {
    pub id: String,
    pub name: String,
    /// "Movie", "Series", "Season", "Episode", "BoxSet", "CollectionFolder",
    /// "Folder", "Video", "MusicVideo", …
    #[serde(rename = "Type")]
    pub kind: String,
    /// For libraries: "movies", "tvshows", "homevideos", "boxsets", …
    pub collection_type: Option<String>,
    pub is_folder: bool,
    pub overview: Option<String>,
    pub taglines: Vec<String>,
    pub production_year: Option<i32>,
    pub premiere_date: Option<String>,
    pub end_date: Option<String>,
    /// "Continuing" or "Ended", for series.
    pub status: Option<String>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub run_time_ticks: Option<i64>,
    pub genres: Vec<String>,
    pub image_tags: HashMap<String, String>,
    pub backdrop_image_tags: Vec<String>,
    pub parent_backdrop_item_id: Option<String>,
    pub parent_backdrop_image_tags: Vec<String>,
    pub parent_thumb_item_id: Option<String>,
    pub parent_thumb_image_tag: Option<String>,
    pub parent_logo_item_id: Option<String>,
    pub parent_logo_image_tag: Option<String>,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub series_primary_image_tag: Option<String>,
    pub season_id: Option<String>,
    pub season_name: Option<String>,
    /// Episode number (or season number, for seasons).
    pub index_number: Option<i32>,
    /// Season number, for episodes.
    pub parent_index_number: Option<i32>,
    pub child_count: Option<u32>,
    pub user_data: Option<UserData>,
    pub people: Vec<Person>,
    pub studios: Vec<NameId>,
    pub media_sources: Vec<MediaSource>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct NameId {
    pub name: String,
    pub id: String,
}

/// Someone credited on an item.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    /// "Actor", "Director", "Writer", …
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    pub primary_image_tag: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageKind {
    Primary,
    Backdrop,
    Thumb,
    Logo,
}

impl ImageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageKind::Primary => "Primary",
            ImageKind::Backdrop => "Backdrop",
            ImageKind::Thumb => "Thumb",
            ImageKind::Logo => "Logo",
        }
    }
}

/// One of an item's images on the server.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageRef {
    pub item_id: String,
    pub kind: ImageKind,
    /// Changes when the image does, so it doubles as a cache key.
    pub tag: Option<String>,
}

impl ImageRef {
    fn new(item_id: &str, kind: ImageKind, tag: Option<&String>) -> Self {
        ImageRef { item_id: item_id.to_string(), kind, tag: tag.cloned() }
    }
}

impl Item {
    pub fn is_series(&self) -> bool {
        self.kind == "Series"
    }

    pub fn is_episode(&self) -> bool {
        self.kind == "Episode"
    }

    /// Something mpv can play, rather than a folder of things.
    pub fn is_playable(&self) -> bool {
        matches!(self.kind.as_str(), "Movie" | "Episode" | "Video" | "MusicVideo" | "Trailer") || !self.is_folder
    }

    /// Movies, shows and folders open a details or library page; episodes
    /// too, but play from cards that resume them.
    pub fn opens_library(&self) -> bool {
        matches!(self.kind.as_str(), "CollectionFolder" | "Folder" | "BoxSet" | "UserView")
    }

    pub fn resume_seconds(&self) -> f64 {
        self.user_data.as_ref().map(|u| u.playback_position_ticks as f64 / TICKS_PER_SECOND).unwrap_or(0.0)
    }

    pub fn played(&self) -> bool {
        self.user_data.as_ref().is_some_and(|u| u.played)
    }

    pub fn favorite(&self) -> bool {
        self.user_data.as_ref().is_some_and(|u| u.is_favorite)
    }

    /// How far into it the user is (0–1), when started and not finished.
    pub fn progress(&self) -> Option<f32> {
        let u = self.user_data.as_ref()?;
        if u.playback_position_ticks <= 0 {
            return None;
        }
        let total = self.run_time_ticks.filter(|t| *t > 0)?;
        Some((u.playback_position_ticks as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }

    pub fn runtime_minutes(&self) -> Option<i64> {
        self.run_time_ticks.map(|t| (t as f64 / TICKS_PER_SECOND / 60.0).round() as i64).filter(|m| *m > 0)
    }

    /// "S1:E4", for episodes.
    pub fn episode_code(&self) -> Option<String> {
        match (self.parent_index_number, self.index_number) {
            (Some(s), Some(e)) => Some(format!("S{s}:E{e}")),
            (None, Some(e)) if self.is_episode() => Some(format!("E{e}")),
            _ => None,
        }
    }

    /// The poster: an item's own Primary, or its series' for an episode
    /// that has none worth showing in a portrait card.
    pub fn poster(&self) -> Option<ImageRef> {
        if self.is_episode() {
            if let (Some(sid), Some(tag)) = (&self.series_id, &self.series_primary_image_tag) {
                return Some(ImageRef::new(sid, ImageKind::Primary, Some(tag)));
            }
        }
        self.image_tags
            .get("Primary")
            .map(|t| ImageRef::new(&self.id, ImageKind::Primary, Some(t)))
    }

    /// A 16:9 picture, as Jellyfin's web client picks one for Continue
    /// Watching and Next Up: the item's thumb, its series' thumb, a
    /// backdrop, then (episodes' Primary is a screenshot) the Primary.
    pub fn landscape(&self) -> Option<ImageRef> {
        if let Some(t) = self.image_tags.get("Thumb") {
            return Some(ImageRef::new(&self.id, ImageKind::Thumb, Some(t)));
        }
        if self.is_episode() {
            if let Some(t) = self.image_tags.get("Primary") {
                return Some(ImageRef::new(&self.id, ImageKind::Primary, Some(t)));
            }
        }
        if let (Some(id), Some(t)) = (&self.parent_thumb_item_id, &self.parent_thumb_image_tag) {
            return Some(ImageRef::new(id, ImageKind::Thumb, Some(t)));
        }
        if let Some(b) = self.backdrop() {
            return Some(b);
        }
        self.image_tags
            .get("Primary")
            .map(|t| ImageRef::new(&self.id, ImageKind::Primary, Some(t)))
    }

    /// A full-width background: the item's own backdrop or its parent's.
    pub fn backdrop(&self) -> Option<ImageRef> {
        if let Some(t) = self.backdrop_image_tags.first() {
            return Some(ImageRef::new(&self.id, ImageKind::Backdrop, Some(t)));
        }
        match (&self.parent_backdrop_item_id, self.parent_backdrop_image_tags.first()) {
            (Some(id), Some(t)) => Some(ImageRef::new(id, ImageKind::Backdrop, Some(t))),
            _ => None,
        }
    }

    /// The title art (a transparent logo), the item's or its series'.
    pub fn logo(&self) -> Option<ImageRef> {
        if let Some(t) = self.image_tags.get("Logo") {
            return Some(ImageRef::new(&self.id, ImageKind::Logo, Some(t)));
        }
        match (&self.parent_logo_item_id, &self.parent_logo_image_tag) {
            (Some(id), Some(t)) => Some(ImageRef::new(id, ImageKind::Logo, Some(t))),
            _ => None,
        }
    }
}

impl Person {
    pub fn image(&self) -> Option<ImageRef> {
        self.primary_image_tag
            .as_ref()
            .map(|t| ImageRef::new(&self.id, ImageKind::Primary, Some(t)))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct ItemsPage {
    items: Vec<Item>,
    total_record_count: usize,
}

/// What cards and pages need of every item.
const FIELDS: &str = "Overview,Genres,PrimaryImageAspectRatio,ProductionYear,OfficialRating,CommunityRating,ChildCount";
const IMAGES: &str = "enableImageTypes=Primary,Backdrop,Thumb,Logo&imageTypeLimit=1&enableUserData=true";

fn query(account: &Account, path: &str, params: &str) -> String {
    let sep = if params.is_empty() { "" } else { "&" };
    format!("{}{path}?{params}{sep}fields={FIELDS}&{IMAGES}", account.url)
}

/// Library kinds this app has nothing to play in.
fn is_video_library(view: &Item) -> bool {
    !matches!(
        view.collection_type.as_deref(),
        Some("books" | "music" | "playlists" | "livetv" | "photos" | "musicvideos_hidden")
    )
}

/// The user's libraries (Movies, Shows, …), books and music left out.
pub async fn libraries(account: &Account) -> Result<Vec<Item>, String> {
    let route = UserRoute {
        current: format!("{}/UserViews?userId={}", account.url, account.user_id),
        legacy: format!("{}/Views", account.user()),
    };
    let page: ItemsPage = route.get(account).await?;
    Ok(page.items.into_iter().filter(is_video_library).collect())
}

/// Continue Watching: started and not finished, most recent first.
pub async fn resume(account: &Account, limit: usize) -> Result<Vec<Item>, String> {
    let params = format!("limit={limit}&mediaTypes=Video&recursive=true");
    let route = UserRoute {
        current: query(account, "/UserItems/Resume", &format!("userId={}&{params}", account.user_id)),
        legacy: query(account, &format!("/Users/{}/Items/Resume", account.user_id), &params),
    };
    let page: ItemsPage = route.get(account).await?;
    Ok(page.items)
}

/// Next Up: the next episode of each show the user is watching.
pub async fn next_up(account: &Account, limit: usize, series_id: Option<&str>) -> Result<Vec<Item>, String> {
    let mut params = format!("userId={}&limit={limit}&enableResumable=false&disableFirstEpisode=false", account.user_id);
    if let Some(s) = series_id {
        params.push_str(&format!("&seriesId={s}"));
    }
    let page: ItemsPage = get_json(&query(account, "/Shows/NextUp", &params), &account.auth()).await?;
    Ok(page.items)
}

/// Recently added to one library. Episodes of the same show are grouped
/// into the show, like Jellyfin's home screen.
pub async fn latest(account: &Account, library_id: &str, limit: usize) -> Result<Vec<Item>, String> {
    account
        .items_route("/Latest", &format!("parentId={library_id}&limit={limit}&groupItems=true"))
        .get(account)
        .await
}

/// The featured banner: random movies and shows that have a backdrop.
pub async fn featured(account: &Account, limit: usize) -> Result<Vec<Item>, String> {
    let route = account.items_route(
        "",
        &format!("includeItemTypes=Movie,Series&recursive=true&sortBy=Random&imageTypes=Backdrop&hasOverview=true&isPlayed=false&limit={limit}"),
    );
    let page: ItemsPage = route.get(account).await?;
    if !page.items.is_empty() {
        return Ok(page.items);
    }
    // Everything watched: random picks from all of it then.
    let route = account.items_route(
        "",
        &format!("includeItemTypes=Movie,Series&recursive=true&sortBy=Random&imageTypes=Backdrop&limit={limit}"),
    );
    let page: ItemsPage = route.get(account).await?;
    Ok(page.items)
}

/// One item, with what a details page shows (cast, studios, sources).
pub async fn item(account: &Account, id: &str) -> Result<Item, String> {
    let tail = format!("fields={FIELDS},People,Studios,Taglines,MediaSources,DateCreated&{IMAGES}");
    let route = UserRoute {
        current: format!("{}/Items/{id}?userId={}&{tail}", account.url, account.user_id),
        legacy: format!("{}/Items/{id}?{tail}", account.user()),
    };
    route.get(account).await
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Name,
    DateAdded,
    ReleaseDate,
    Rating,
}

impl SortBy {
    pub const ALL: [SortBy; 4] = [SortBy::Name, SortBy::DateAdded, SortBy::ReleaseDate, SortBy::Rating];

    fn params(self) -> &'static str {
        match self {
            SortBy::Name => "sortBy=SortName&sortOrder=Ascending",
            SortBy::DateAdded => "sortBy=DateCreated,SortName&sortOrder=Descending",
            SortBy::ReleaseDate => "sortBy=PremiereDate,ProductionYear,SortName&sortOrder=Descending",
            SortBy::Rating => "sortBy=CommunityRating,SortName&sortOrder=Descending",
        }
    }
}

/// What a library page lists: a library or folder, its order, and an
/// optional search within it.
#[derive(Clone, Debug)]
pub struct LibraryQuery {
    pub parent: Item,
    pub sort: SortBy,
    pub search: String,
}

impl LibraryQuery {
    /// Movie and show libraries list every movie/show inside (however the
    /// folders are laid out); others list the folder's direct children,
    /// or anything inside when searching.
    fn params(&self) -> String {
        let kinds = match self.parent.collection_type.as_deref() {
            Some("movies") => Some("Movie"),
            Some("tvshows") => Some("Series"),
            Some("boxsets") => Some("BoxSet"),
            Some("musicvideos") => Some("MusicVideo"),
            _ => None,
        };
        let mut params = format!("parentId={}&{}", self.parent.id, self.sort.params());
        match kinds {
            Some(k) => params.push_str(&format!("&includeItemTypes={k}&recursive=true")),
            None => params.push_str("&excludeItemTypes=Audio,Book,Photo,MusicAlbum"),
        }
        let search = self.search.trim();
        if !search.is_empty() {
            let term: String = url::form_urlencoded::byte_serialize(search.as_bytes()).collect();
            params.push_str(&format!("&searchTerm={term}"));
            if kinds.is_none() {
                params.push_str("&recursive=true");
            }
        }
        params
    }
}

/// One page of a library: its items from `start`, and how many there are
/// in all.
pub async fn library_page(account: &Account, q: &LibraryQuery, start: usize, limit: usize) -> Result<(Vec<Item>, usize), String> {
    let params = format!("{}&startIndex={start}&limit={limit}", q.params());
    let page: ItemsPage = account.items_route("", &params).get(account).await?;
    Ok((page.items, page.total_record_count))
}

/// Where the first item whose name starts at `letter` or later is, in a
/// library sorted by name: the number sorted before it. Jellyfin compares
/// sort names, which are lower case and without a leading "The".
pub async fn library_position(account: &Account, q: &LibraryQuery, letter: char) -> Result<usize, String> {
    let before = letter.to_lowercase().to_string();
    let params = format!("{}&nameLessThan={before}&limit=0", q.params());
    let page: ItemsPage = account.items_route("", &params).get(account).await?;
    Ok(page.total_record_count)
}

pub async fn seasons(account: &Account, series_id: &str) -> Result<Vec<Item>, String> {
    let url = query(account, &format!("/Shows/{series_id}/Seasons"), &format!("userId={}", account.user_id));
    let page: ItemsPage = get_json(&url, &account.auth()).await?;
    Ok(page.items)
}

pub async fn episodes(account: &Account, series_id: &str, season_id: &str) -> Result<Vec<Item>, String> {
    let url = query(
        account,
        &format!("/Shows/{series_id}/Episodes"),
        &format!("userId={}&seasonId={season_id}", account.user_id),
    );
    let page: ItemsPage = get_json(&url, &account.auth()).await?;
    Ok(page.items)
}

/// The episode after `episode_id` in its show, across seasons.
pub async fn episode_after(account: &Account, series_id: &str, episode_id: &str) -> Result<Option<Item>, String> {
    let url = query(
        account,
        &format!("/Shows/{series_id}/Episodes"),
        &format!("userId={}&startItemId={episode_id}&limit=2", account.user_id),
    );
    let page: ItemsPage = get_json(&url, &account.auth()).await?;
    Ok(page.items.into_iter().find(|e| e.id != episode_id))
}

/// The episodes either side of `episode_id` in its show, across seasons.
pub async fn episode_neighbours(account: &Account, series_id: &str, episode_id: &str) -> Result<(Option<Item>, Option<Item>), String> {
    let url = query(account, &format!("/Shows/{series_id}/Episodes"), &format!("userId={}", account.user_id));
    let page: ItemsPage = get_json(&url, &account.auth()).await?;
    let Some(i) = page.items.iter().position(|e| e.id == episode_id) else { return Ok((None, None)) };
    let previous = i.checked_sub(1).and_then(|p| page.items.get(p)).cloned();
    Ok((previous, page.items.get(i + 1).cloned()))
}

/// What to play for a show: where the user is up to, or the very first
/// episode.
pub async fn series_start(account: &Account, series_id: &str) -> Result<Option<Item>, String> {
    if let Some(next) = next_up(account, 1, Some(series_id)).await?.into_iter().next() {
        return Ok(Some(next));
    }
    let url = query(
        account,
        &format!("/Shows/{series_id}/Episodes"),
        &format!("userId={}&limit=1", account.user_id),
    );
    let page: ItemsPage = get_json(&url, &account.auth()).await?;
    Ok(page.items.into_iter().next())
}

/// Items like this one.
pub async fn similar(account: &Account, id: &str, limit: usize) -> Result<Vec<Item>, String> {
    let url = query(account, &format!("/Items/{id}/Similar"), &format!("userId={}&limit={limit}", account.user_id));
    let page: ItemsPage = get_json(&url, &account.auth()).await?;
    Ok(page.items)
}

/// Movies, shows and episodes matching `term`.
pub async fn search(account: &Account, term: &str, limit: usize) -> Result<Vec<Item>, String> {
    let term: String = url::form_urlencoded::byte_serialize(term.as_bytes()).collect();
    let route = account.items_route(
        "",
        &format!("searchTerm={term}&includeItemTypes=Movie,Series,Episode,BoxSet&recursive=true&limit={limit}"),
    );
    let page: ItemsPage = route.get(account).await?;
    Ok(page.items)
}

pub async fn set_played(account: &Account, id: &str, played: bool) -> Result<(), String> {
    let route = UserRoute {
        current: format!("{}/UserPlayedItems/{id}?userId={}", account.url, account.user_id),
        legacy: format!("{}/PlayedItems/{id}", account.user()),
    };
    route.set(account, played).await
}

pub async fn set_favorite(account: &Account, id: &str, favorite: bool) -> Result<(), String> {
    let route = UserRoute {
        current: format!("{}/UserFavoriteItems/{id}?userId={}", account.url, account.user_id),
        legacy: format!("{}/FavoriteItems/{id}", account.user()),
    };
    route.set(account, favorite).await
}

/// Downloads an image.
pub async fn image(account: &Account, img: &ImageRef, max_width: u32) -> Result<Vec<u8>, String> {
    let resp = client()
        .get(account.image_url(img, max_width))
        .header("Authorization", account.auth())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

// ── Playback ────────────────────────────────────────────────────────────────

/// One size of an item's trickplay thumbnails: the server makes them ahead
/// of time (a scheduled task, when the library has it turned on), a frame
/// every `interval` ms, packed `tile_width` × `tile_height` to a JPEG sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TrickplayInfo {
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub thumbnail_count: u32,
    /// Milliseconds between thumbnails.
    pub interval: u32,
}

impl TrickplayInfo {
    pub fn per_sheet(&self) -> u32 {
        self.tile_width * self.tile_height
    }

    /// The thumbnail for `seconds` in: which sheet, and which tile on it
    /// (counted across, then down), as jellyfin-web works it out.
    pub fn locate(&self, seconds: f64) -> Option<(u32, u32)> {
        if self.interval == 0 || self.per_sheet() == 0 || self.thumbnail_count == 0 {
            return None;
        }
        let n = ((seconds.max(0.0) * 1000.0) as u64 / self.interval as u64).min(self.thumbnail_count as u64 - 1) as u32;
        Some((n / self.per_sheet(), n % self.per_sheet()))
    }

    /// How many thumbnails sheet `index` holds (the last may be short).
    pub fn tiles_on(&self, index: u32) -> u32 {
        self.thumbnail_count.saturating_sub(index * self.per_sheet()).min(self.per_sheet())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Chapter {
    pub start_position_ticks: i64,
    pub name: Option<String>,
}

impl Chapter {
    pub fn start(&self) -> f64 {
        self.start_position_ticks as f64 / TICKS_PER_SECOND
    }
}

/// What the player shows on its seek bar: trickplay thumbnails (per media
/// source, per width) and chapters. Asked for only for the item playing:
/// the server looks the thumbnails up for each item it's asked about.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SeekExtras {
    trickplay: HashMap<String, HashMap<String, TrickplayInfo>>,
    pub chapters: Vec<Chapter>,
}

impl SeekExtras {
    /// The thumbnails for `source_id` to use: the widest no wider than
    /// `max_width`, else the narrowest (jellyfin-web's choice, with
    /// `max_width` a fifth of the screen).
    pub fn trickplay(&self, source_id: &str, max_width: u32) -> Option<TrickplayInfo> {
        // The keys are GUIDs, which may come with or without dashes.
        let plain = |id: &str| id.replace('-', "").to_lowercase();
        let sizes = self
            .trickplay
            .iter()
            .find(|(k, _)| plain(k) == plain(source_id))
            .or_else(|| (self.trickplay.len() == 1).then(|| self.trickplay.iter().next()).flatten())?
            .1;
        let fits = sizes.values().filter(|i| i.width <= max_width).max_by_key(|i| i.width);
        fits.or_else(|| sizes.values().min_by_key(|i| i.width)).copied().filter(|i| i.locate(0.0).is_some())
    }
}

/// The trickplay manifest and chapters of one item. Servers before 10.9
/// (no trickplay) or without them for this item give empty ones.
pub async fn seek_extras(account: &Account, id: &str) -> SeekExtras {
    let tail = "fields=Trickplay,Chapters&enableImages=false&enableUserData=false";
    let route = UserRoute {
        current: format!("{}/Items/{id}?userId={}&{tail}", account.url, account.user_id),
        legacy: format!("{}/Items/{id}?{tail}", account.user()),
    };
    route.get(account).await.unwrap_or_else(|e| {
        log::info!("no trickplay or chapters for {id}: {e}");
        SeekExtras::default()
    })
}

/// One sheet of trickplay thumbnails (a JPEG). The server sends a file it
/// made ahead of time; nothing is generated on request.
pub async fn trickplay_sheet(account: &Account, item_id: &str, source_id: &str, width: u32, index: u32) -> Result<Vec<u8>, String> {
    let url = format!("{}/Videos/{item_id}/Trickplay/{width}/{index}.jpg?mediaSourceId={source_id}", account.url);
    let resp = client()
        .get(url)
        .header("Authorization", account.auth())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct MediaSource {
    pub id: String,
    pub name: Option<String>,
    pub container: Option<String>,
    pub media_streams: Vec<MediaStream>,
    pub default_audio_stream_index: Option<i64>,
    pub default_subtitle_stream_index: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct MediaStream {
    /// "Video", "Audio", "Subtitle", "Attachment", …
    #[serde(rename = "Type")]
    pub kind: String,
    pub index: i64,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub display_title: Option<String>,
    pub is_external: bool,
    pub delivery_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlaybackInfo {
    pub media_sources: Vec<MediaSource>,
    pub play_session_id: Option<String>,
}

/// Asks the server how to play an item; the reply names its file(s), their
/// streams, the user's preferred tracks and a play session id.
pub async fn playback_info(account: &Account, id: &str) -> Result<PlaybackInfo, String> {
    get_json(&format!("{}/Items/{id}/PlaybackInfo?userId={}", account.url, account.user_id), &account.auth()).await
}

impl MediaSource {
    /// mpv's track ids for the server's preferred audio and subtitle
    /// streams. Subtitle `Some(0)` means "off"; external subtitles aren't
    /// counted, they're added (and picked) separately.
    pub fn track_choice(&self) -> (Option<i64>, Option<i64>) {
        let embedded = |kind: &str| -> Vec<i64> {
            self.media_streams.iter().filter(|s| s.kind == kind && !s.is_external).map(|s| s.index).collect()
        };
        let audio = self.default_audio_stream_index.and_then(|i| crate::mpv::track_id(&embedded("Audio"), i));
        let subtitle = match self.default_subtitle_stream_index {
            Some(-1) => Some(0),
            Some(i) => crate::mpv::track_id(&embedded("Subtitle"), i),
            None => None,
        };
        (audio, subtitle)
    }

    /// Subtitle files kept next to the video on the server.
    pub fn external_subs(&self, account: &Account, item_id: &str) -> Vec<crate::mpv::ExternalSub> {
        self.media_streams
            .iter()
            .filter(|s| s.kind == "Subtitle" && s.is_external)
            .map(|s| {
                let url = match &s.delivery_url {
                    Some(u) if u.starts_with("http") => u.clone(),
                    Some(u) => format!("{}{u}", account.url),
                    None => format!(
                        "{}/Videos/{item_id}/{}/Subtitles/{}/Stream.{}",
                        account.url,
                        self.id,
                        s.index,
                        s.codec.as_deref().unwrap_or("srt")
                    ),
                };
                crate::mpv::ExternalSub {
                    url,
                    title: s.display_title.clone().unwrap_or_default(),
                    lang: s.language.clone().unwrap_or_default(),
                    select: self.default_subtitle_stream_index == Some(s.index),
                }
            })
            .collect()
    }
}

/// A part of an episode or movie that can be skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Intro,
    /// Credits (Jellyfin calls them Outro).
    Credits,
    Recap,
    Preview,
    Commercial,
}

impl SegmentKind {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "Intro" | "Introduction" => Some(SegmentKind::Intro),
            "Outro" | "Credits" => Some(SegmentKind::Credits),
            "Recap" => Some(SegmentKind::Recap),
            "Preview" => Some(SegmentKind::Preview),
            "Commercial" => Some(SegmentKind::Commercial),
            _ => None,
        }
    }

    /// What the skip prompt and notice call it.
    pub fn label(self) -> &'static str {
        match self {
            SegmentKind::Intro => "Intro",
            SegmentKind::Credits => "Credits",
            SegmentKind::Recap => "Recap",
            SegmentKind::Preview => "Preview",
            SegmentKind::Commercial => "Ad",
        }
    }
}

/// Where a skippable part starts and ends, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct MediaSegmentDto {
    #[serde(rename = "Type")]
    kind: String,
    start_ticks: i64,
    end_ticks: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct MediaSegmentPage {
    items: Vec<MediaSegmentDto>,
}

/// The older Intro Skipper plugin's answer (Jellyfin 10.9 and before), in
/// seconds.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct IntroSkipperSegment {
    valid: bool,
    intro_start: f64,
    intro_end: f64,
}

fn keep(mut segments: Vec<Segment>) -> Vec<Segment> {
    // A few seconds aren't worth a prompt.
    segments.retain(|s| s.end - s.start >= 1.0);
    segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    segments
}

fn parse_media_segments(page: MediaSegmentPage) -> Vec<Segment> {
    keep(
        page.items
            .into_iter()
            .filter_map(|s| {
                Some(Segment {
                    kind: SegmentKind::parse(&s.kind)?,
                    start: s.start_ticks as f64 / TICKS_PER_SECOND,
                    end: s.end_ticks as f64 / TICKS_PER_SECOND,
                })
            })
            .collect(),
    )
}

fn parse_intro_skipper(map: HashMap<String, IntroSkipperSegment>) -> Vec<Segment> {
    keep(
        map.into_iter()
            .filter(|(_, s)| s.valid)
            .filter_map(|(name, s)| {
                Some(Segment { kind: SegmentKind::parse(&name)?, start: s.intro_start, end: s.intro_end })
            })
            .collect(),
    )
}

/// The parts of an item that can be skipped: Jellyfin's media segments
/// (10.10+, filled in by Intro Skipper and similar plugins), else the older
/// Intro Skipper plugin's own endpoint. None found, or no plugin: empty.
pub async fn segments(account: &Account, item_id: &str) -> Vec<Segment> {
    let url = format!(
        "{}/MediaSegments/{item_id}?includeSegmentTypes=Intro&includeSegmentTypes=Outro&includeSegmentTypes=Recap&includeSegmentTypes=Preview&includeSegmentTypes=Commercial",
        account.url
    );
    match get_json::<MediaSegmentPage>(&url, &account.auth()).await {
        Ok(page) => return parse_media_segments(page),
        Err(e) if !not_found(&e) => {
            log::info!("no media segments for {item_id}: {e}");
            return Vec::new();
        }
        Err(_) => {}
    }
    let url = format!("{}/Episode/{item_id}/IntroSkipperSegments", account.url);
    match get_json::<HashMap<String, IntroSkipperSegment>>(&url, &account.auth()).await {
        Ok(map) => parse_intro_skipper(map),
        Err(e) => {
            log::debug!("no Intro Skipper segments for {item_id}: {e}");
            Vec::new()
        }
    }
}

/// A playback report's body.
#[derive(Clone, Debug)]
pub struct Report {
    pub item_id: String,
    pub media_source_id: String,
    pub play_session_id: String,
    pub position: f64,
    pub paused: bool,
}

impl Report {
    fn body(&self) -> serde_json::Value {
        serde_json::json!({
            "ItemId": self.item_id,
            "MediaSourceId": self.media_source_id,
            "PlaySessionId": self.play_session_id,
            "PositionTicks": (self.position * TICKS_PER_SECOND) as i64,
            "IsPaused": self.paused,
            "PlayMethod": "DirectPlay",
            "CanSeek": true,
        })
    }
}

pub async fn report_started(account: &Account, r: Report) -> Result<(), String> {
    send(client().post(format!("{}/Sessions/Playing", account.url)).json(&r.body()), &account.auth()).await
}

pub async fn report_progress(account: &Account, r: Report) -> Result<(), String> {
    send(client().post(format!("{}/Sessions/Playing/Progress", account.url)).json(&r.body()), &account.auth()).await
}

pub async fn report_stopped(account: &Account, r: Report) -> Result<(), String> {
    send(client().post(format!("{}/Sessions/Playing/Stopped", account.url)).json(&r.body()), &account.auth()).await
}

/// The same stop report, sent before the app exits (no runtime left to
/// await on).
pub fn report_stopped_now(account: &Account, r: &Report) {
    let result = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .and_then(|c| {
            c.post(format!("{}/Sessions/Playing/Stopped", account.url))
                .header("Authorization", account.auth())
                .json(&r.body())
                .send()
        });
    if let Err(e) = result {
        log::warn!("couldn't report playback stopped: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trickplay_thumbnails_are_found_like_jellyfin_web() {
        // Jellyfin's defaults: every 10 s, 10 × 10 to a sheet.
        let info = TrickplayInfo { width: 320, height: 180, tile_width: 10, tile_height: 10, thumbnail_count: 720, interval: 10_000 };
        assert_eq!(info.locate(0.0), Some((0, 0)));
        assert_eq!(info.locate(9.9), Some((0, 0)));
        assert_eq!(info.locate(10.0), Some((0, 1)));
        assert_eq!(info.locate(1005.0), Some((1, 0)));
        // Past the end: the last thumbnail.
        assert_eq!(info.locate(1e9), Some((7, 19)));
        assert_eq!(info.tiles_on(7), 20);
        assert_eq!(info.tiles_on(0), 100);

        let json = r#"{"Trickplay":{"1b2c3d4e5f60718293a4b5c6d7e8f901":{
            "320":{"Width":320,"Height":180,"TileWidth":10,"TileHeight":10,"ThumbnailCount":720,"Interval":10000},
            "640":{"Width":640,"Height":360,"TileWidth":10,"TileHeight":10,"ThumbnailCount":720,"Interval":10000}}},
            "Chapters":[{"StartPositionTicks":0,"Name":"Opening"}]}"#;
        let extras: SeekExtras = serde_json::from_str(json).unwrap();
        let source = "1b2c3d4e-5f60-7182-93a4-b5c6d7e8f901";
        assert_eq!(extras.trickplay(source, 400).map(|i| i.width), Some(320));
        assert_eq!(extras.trickplay(source, 2000).map(|i| i.width), Some(640));
        assert_eq!(extras.trickplay(source, 100).map(|i| i.width), Some(320));
        assert_eq!(extras.chapters[0].name.as_deref(), Some("Opening"));
        assert!(SeekExtras::default().trickplay(source, 400).is_none());
    }

    #[test]
    fn candidate_urls_strip_web_paths() {
        assert_eq!(candidate_urls("jf.local:8096/web/#/home"), vec!["https://jf.local:8096", "http://jf.local:8096"]);
        assert_eq!(candidate_urls("http://x/"), vec!["http://x"]);
        assert!(candidate_urls("  ").is_empty());
    }

    fn source(json: serde_json::Value) -> MediaSource {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn preferred_tracks_map_to_mpv_ids() {
        let s = source(serde_json::json!({
            "Id": "m",
            "DefaultAudioStreamIndex": 2,
            "DefaultSubtitleStreamIndex": 4,
            "MediaStreams": [
                {"Type": "Video", "Index": 0},
                {"Type": "Audio", "Index": 1},
                {"Type": "Audio", "Index": 2},
                {"Type": "Subtitle", "Index": 3},
                {"Type": "Subtitle", "Index": 4},
                {"Type": "Subtitle", "Index": 5, "IsExternal": true, "Codec": "srt"}
            ]
        }));
        assert_eq!(s.track_choice(), (Some(2), Some(2)));
        let off = MediaSource { default_subtitle_stream_index: Some(-1), ..s.clone() };
        assert_eq!(off.track_choice().1, Some(0));
        // An external pick isn't an embedded track; it's selected on add.
        let ext = MediaSource { default_subtitle_stream_index: Some(5), ..s };
        assert_eq!(ext.track_choice().1, None);
        let account = Account {
            url: "http://jf".into(),
            server_id: "s".into(),
            server_name: "S".into(),
            user_id: "u".into(),
            user_name: "U".into(),
            token: "t".into(),
        };
        let subs = ext.external_subs(&account, "i");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].url, "http://jf/Videos/i/m/Subtitles/5/Stream.srt");
        assert!(subs[0].select);
    }

    #[test]
    fn landscape_prefers_thumbs_then_screenshots() {
        let ep: Item = serde_json::from_value(serde_json::json!({
            "Id": "e", "Type": "Episode",
            "ImageTags": {"Primary": "p"},
            "ParentThumbItemId": "s", "ParentThumbImageTag": "t",
            "SeriesId": "s", "SeriesPrimaryImageTag": "sp"
        }))
        .unwrap();
        assert_eq!(ep.landscape().unwrap().item_id, "e");
        assert_eq!(ep.poster().unwrap(), ImageRef::new("s", ImageKind::Primary, Some(&"sp".to_string())));
        let movie: Item = serde_json::from_value(serde_json::json!({
            "Id": "m", "Type": "Movie", "ImageTags": {"Primary": "p"}, "BackdropImageTags": ["b"]
        }))
        .unwrap();
        assert_eq!(movie.landscape().unwrap().kind, ImageKind::Backdrop);
    }

    #[test]
    fn media_segments_parse_in_seconds_and_order() {
        let page: MediaSegmentPage = serde_json::from_value(serde_json::json!({
            "Items": [
                {"Type": "Outro", "StartTicks": 12_000_000_000i64, "EndTicks": 13_000_000_000i64},
                {"Type": "Intro", "StartTicks": 50_000_000, "EndTicks": 900_000_000},
                {"Type": "Unknown", "StartTicks": 0, "EndTicks": 100_000_000},
                {"Type": "Recap", "StartTicks": 0, "EndTicks": 5_000_000}
            ],
            "TotalRecordCount": 4
        }))
        .unwrap();
        let s = parse_media_segments(page);
        assert_eq!(
            s,
            vec![
                Segment { kind: SegmentKind::Intro, start: 5.0, end: 90.0 },
                Segment { kind: SegmentKind::Credits, start: 1200.0, end: 1300.0 },
            ]
        );
    }

    #[test]
    fn old_intro_skipper_answers_parse() {
        let map: HashMap<String, IntroSkipperSegment> = serde_json::from_value(serde_json::json!({
            "Introduction": {"EpisodeId": "e", "Valid": true, "IntroStart": 12.5, "IntroEnd": 80.0,
                             "ShowSkipPromptAt": 7.5, "HideSkipPromptAt": 22.5},
            "Credits": {"EpisodeId": "e", "Valid": false, "IntroStart": 0.0, "IntroEnd": 0.0}
        }))
        .unwrap();
        assert_eq!(parse_intro_skipper(map), vec![Segment { kind: SegmentKind::Intro, start: 12.5, end: 80.0 }]);
    }

    #[test]
    fn progress_needs_a_runtime() {
        let mut i = Item {
            run_time_ticks: Some(100),
            user_data: Some(UserData { playback_position_ticks: 25, ..Default::default() }),
            ..Default::default()
        };
        assert_eq!(i.progress(), Some(0.25));
        i.run_time_ticks = None;
        assert_eq!(i.progress(), None);
    }
}
