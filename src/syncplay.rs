//! SyncPlay, Jellyfin's "watch together": the server's side of it.
//!
//! This is the protocol as jellyfin-web speaks it (its `plugins/syncPlay`):
//! REST requests under `/SyncPlay/...` for everything a member asks for,
//! and the server's answers pushed over the session's WebSocket
//! (`SyncPlayCommand` and `SyncPlayGroupUpdate` messages). Commands carry
//! a server time (`When`) at which every member acts, so each client
//! measures its clock's offset from the server's (`/GetUtcTime`, as
//! jellyfin-web's TimeSync does) and schedules them on its own clock.
//!
//! What the app does with all this (the player's side) is in
//! `app/syncplay.rs`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::json;

use crate::jellyfin::{self, Account};

/// Jellyfin's ticks: 100 ns.
pub const TICKS_PER_MS: f64 = 10_000.0;

pub fn seconds_to_ticks(seconds: f64) -> i64 {
    (seconds * 1000.0 * TICKS_PER_MS).round() as i64
}

pub fn ticks_to_seconds(ticks: i64) -> f64 {
    ticks as f64 / TICKS_PER_MS / 1000.0
}

// ── Time ─────────────────────────────────────────────────────────────────────

/// Milliseconds since 1970 on this machine's clock.
pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Days from 1970-01-01 to a date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parses the server's UTC times, such as `2026-10-22T20:01:46.1234567Z`,
/// to milliseconds since 1970.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00")).unwrap_or(s);
    let (date, time) = s.split_once('T')?;
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
    let mut t = hms.splitn(3, ':').map(|p| p.parse::<i64>().ok());
    let (h, mi, se) = (t.next()??, t.next()??, t.next()??);
    let frac_ms = if frac.is_empty() {
        0.0
    } else {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<f64>().ok()? / 10f64.powi(digits.len() as i32) * 1000.0
    };
    let secs = days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + se;
    Some(secs as f64 * 1000.0 + frac_ms)
}

/// Formats milliseconds since 1970 as `toISOString` does.
pub fn format_time(ms: f64) -> String {
    let ms = ms.round() as i64;
    let (secs, millis) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z", rest / 3600, rest / 60 % 60, rest % 60)
}

/// One `/GetUtcTime` round trip: when the request left and the answer
/// came back here, and when the server got it and answered (all ms).
#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    pub request_sent: f64,
    pub request_received: f64,
    pub response_sent: f64,
    pub response_received: f64,
}

impl Measurement {
    /// Server clock minus ours.
    pub fn offset(&self) -> f64 {
        ((self.request_received - self.request_sent) + (self.response_sent - self.response_received)) / 2.0
    }

    /// The round trip, less the server's own time.
    pub fn delay(&self) -> f64 {
        (self.response_received - self.request_sent) - (self.response_sent - self.request_received)
    }

    pub fn ping(&self) -> f64 {
        (self.delay() / 2.0).round()
    }
}

/// jellyfin-web's TimeSync: the last 8 measurements, trusting the one
/// with the shortest round trip. Three quick pings, then one a minute.
#[derive(Debug, Default)]
pub struct TimeSync {
    measurements: Vec<Measurement>,
    best: Option<Measurement>,
    pub pings: u32,
}

pub const GREEDY_PINGS: u32 = 3;
pub const PING_GREEDY: Duration = Duration::from_secs(1);
pub const PING_LOW_PROFILE: Duration = Duration::from_secs(60);

impl TimeSync {
    pub fn add(&mut self, m: Measurement) {
        self.measurements.push(m);
        if self.measurements.len() > 8 {
            self.measurements.remove(0);
        }
        self.best = self.measurements.iter().copied().min_by(|a, b| a.delay().total_cmp(&b.delay()));
        self.pings += 1;
    }

    pub fn ready(&self) -> bool {
        self.best.is_some()
    }

    pub fn offset(&self) -> f64 {
        self.best.map_or(0.0, |m| m.offset())
    }

    pub fn ping(&self) -> f64 {
        self.best.map_or(0.0, |m| m.ping())
    }

    /// When to measure again.
    pub fn next_interval(&self) -> Duration {
        if self.pings >= GREEDY_PINGS {
            PING_LOW_PROFILE
        } else {
            PING_GREEDY
        }
    }

    /// Back to quick pings, as on joining a group.
    pub fn force_update(&mut self) {
        self.pings = 0;
    }

    pub fn to_local(&self, server_ms: f64) -> f64 {
        server_ms - self.offset()
    }

    pub fn to_server(&self, local_ms: f64) -> f64 {
        local_ms + self.offset()
    }
}

// ── What the server sends ────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct GroupInfo {
    pub group_id: String,
    pub group_name: String,
    pub state: String,
    pub participants: Vec<String>,
    pub last_updated_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    Unpause,
    Pause,
    Stop,
    Seek,
}

/// A playback command for the whole group, at a server time.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub group_id: String,
    pub playlist_item_id: String,
    pub kind: CommandKind,
    /// Server ms.
    pub when: f64,
    pub emitted_at: f64,
    pub position_ticks: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawCommand {
    #[serde(default)]
    group_id: String,
    #[serde(default)]
    playlist_item_id: String,
    command: String,
    when: String,
    emitted_at: String,
    #[serde(default)]
    position_ticks: Option<i64>,
}

impl Command {
    fn from_raw(raw: RawCommand) -> Option<Self> {
        let kind = match raw.command.as_str() {
            "Unpause" => CommandKind::Unpause,
            "Pause" => CommandKind::Pause,
            "Stop" => CommandKind::Stop,
            "Seek" => CommandKind::Seek,
            _ => return None,
        };
        Some(Command {
            group_id: raw.group_id,
            playlist_item_id: raw.playlist_item_id,
            kind,
            when: parse_time(&raw.when)?,
            emitted_at: parse_time(&raw.emitted_at)?,
            position_ticks: raw.position_ticks,
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct QueueItem {
    pub item_id: String,
    pub playlist_item_id: String,
}

/// The group's play queue, sent whenever it changes.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlayQueue {
    pub reason: String,
    pub last_update: String,
    pub playlist: Vec<QueueItem>,
    pub playing_item_index: i32,
    pub start_position_ticks: i64,
    pub is_playing: bool,
}

impl PlayQueue {
    pub fn last_update_ms(&self) -> f64 {
        parse_time(&self.last_update).unwrap_or(0.0)
    }

    pub fn current(&self) -> Option<&QueueItem> {
        usize::try_from(self.playing_item_index).ok().and_then(|i| self.playlist.get(i))
    }
}

/// `SyncPlayGroupUpdate`'s kinds, with their data.
#[derive(Clone, Debug, PartialEq)]
pub enum GroupUpdate {
    UserJoined(String),
    UserLeft(String),
    GroupJoined(GroupInfo),
    GroupUpdate(GroupInfo),
    GroupLeft,
    NotInGroup,
    StateUpdate { state: String, reason: String },
    PlayQueue(PlayQueue),
    GroupDoesNotExist,
    CreateGroupDenied,
    JoinGroupDenied,
    LibraryAccessDenied,
    SyncPlayIsDisabled,
}

fn group_update(value: &serde_json::Value) -> Option<GroupUpdate> {
    let data = value.get("Data").cloned().unwrap_or(serde_json::Value::Null);
    let text = || data.as_str().unwrap_or_default().to_string();
    Some(match value.get("Type")?.as_str()? {
        "UserJoined" => GroupUpdate::UserJoined(text()),
        "UserLeft" => GroupUpdate::UserLeft(text()),
        "GroupJoined" => GroupUpdate::GroupJoined(serde_json::from_value(data).ok()?),
        "GroupUpdate" => GroupUpdate::GroupUpdate(serde_json::from_value(data).ok()?),
        "GroupLeft" => GroupUpdate::GroupLeft,
        "NotInGroup" => GroupUpdate::NotInGroup,
        "StateUpdate" => GroupUpdate::StateUpdate {
            state: data.get("State")?.as_str()?.to_string(),
            reason: data.get("Reason").and_then(|r| r.as_str()).unwrap_or_default().to_string(),
        },
        "PlayQueue" => GroupUpdate::PlayQueue(serde_json::from_value(data).ok()?),
        "GroupDoesNotExist" => GroupUpdate::GroupDoesNotExist,
        "CreateGroupDenied" => GroupUpdate::CreateGroupDenied,
        "JoinGroupDenied" => GroupUpdate::JoinGroupDenied,
        "LibraryAccessDenied" => GroupUpdate::LibraryAccessDenied,
        "SyncPlayIsDisabled" => GroupUpdate::SyncPlayIsDisabled,
        _ => return None,
    })
}

/// What arrives over the WebSocket that SyncPlay cares about.
#[derive(Clone, Debug, PartialEq)]
pub enum Inbound {
    Connected,
    Disconnected,
    Command(Command),
    Group(GroupUpdate),
}

/// Reads one WebSocket message.
pub fn parse_message(text: &str) -> Option<Inbound> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    match v.get("MessageType")?.as_str()? {
        "SyncPlayCommand" => {
            let raw: RawCommand = serde_json::from_value(v.get("Data")?.clone()).ok()?;
            Command::from_raw(raw).map(Inbound::Command)
        }
        "SyncPlayGroupUpdate" => group_update(v.get("Data")?).map(Inbound::Group),
        _ => None,
    }
}

// ── Requests ─────────────────────────────────────────────────────────────────

pub async fn list_groups(account: &Account) -> Result<Vec<GroupInfo>, String> {
    jellyfin::get_json(&format!("{}/SyncPlay/List", account.url), &account.auth()).await
}

async fn post(account: &Account, what: &str, body: serde_json::Value) -> Result<(), String> {
    let req = jellyfin::client().post(format!("{}/SyncPlay/{what}", account.url)).json(&body);
    jellyfin::send(req, &account.auth()).await.map_err(|e| format!("SyncPlay {what}: {e}"))
}

pub async fn new_group(account: &Account, name: &str) -> Result<(), String> {
    post(account, "New", json!({ "GroupName": name })).await
}

pub async fn join(account: &Account, group_id: &str) -> Result<(), String> {
    post(account, "Join", json!({ "GroupId": group_id })).await
}

pub async fn leave(account: &Account) -> Result<(), String> {
    post(account, "Leave", json!({})).await
}

pub async fn set_new_queue(account: &Account, ids: &[String], index: usize, start_ticks: i64) -> Result<(), String> {
    post(
        account,
        "SetNewQueue",
        json!({ "PlayingQueue": ids, "PlayingItemPosition": index, "StartPositionTicks": start_ticks }),
    )
    .await
}

pub async fn unpause(account: &Account) -> Result<(), String> {
    post(account, "Unpause", json!({})).await
}

pub async fn pause(account: &Account) -> Result<(), String> {
    post(account, "Pause", json!({})).await
}

pub async fn seek(account: &Account, ticks: i64) -> Result<(), String> {
    post(account, "Seek", json!({ "PositionTicks": ticks })).await
}

pub async fn next_item(account: &Account, playlist_item_id: &str) -> Result<(), String> {
    post(account, "NextItem", json!({ "PlaylistItemId": playlist_item_id })).await
}

pub async fn previous_item(account: &Account, playlist_item_id: &str) -> Result<(), String> {
    post(account, "PreviousItem", json!({ "PlaylistItemId": playlist_item_id })).await
}

pub async fn set_ignore_wait(account: &Account, ignore: bool) -> Result<(), String> {
    post(account, "SetIgnoreWait", json!({ "IgnoreWait": ignore })).await
}

pub async fn ping(account: &Account, ping_ms: f64) -> Result<(), String> {
    post(account, "Ping", json!({ "Ping": ping_ms as i64 })).await
}

/// Where this member is, for `/SyncPlay/Buffering` (or `Ready`, when
/// `ready`).
#[derive(Clone, Debug)]
pub struct Position {
    /// Server ms.
    pub when: f64,
    pub position_ticks: i64,
    pub is_playing: bool,
    pub playlist_item_id: String,
}

pub async fn report(account: &Account, ready: bool, p: Position) -> Result<(), String> {
    let body = json!({
        "When": format_time(p.when),
        "PositionTicks": p.position_ticks,
        "IsPlaying": p.is_playing,
        "PlaylistItemId": p.playlist_item_id,
    });
    post(account, if ready { "Ready" } else { "Buffering" }, body).await
}

pub async fn measure(account: &Account) -> Result<Measurement, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct UtcTime {
        request_reception_time: String,
        response_transmission_time: String,
    }
    let request_sent = now_ms();
    let t: UtcTime = jellyfin::get_json(&format!("{}/GetUtcTime", account.url), &account.auth()).await?;
    let response_received = now_ms();
    Ok(Measurement {
        request_sent,
        request_received: parse_time(&t.request_reception_time).ok_or("bad server time")?,
        response_sent: parse_time(&t.response_transmission_time).ok_or("bad server time")?,
        response_received,
    })
}

// ── WebSocket ────────────────────────────────────────────────────────────────

/// The session's WebSocket address, as jellyfin-web opens it, for a
/// server's base URL and an access token.
pub fn socket_url(base: &str, token: &str) -> String {
    let ws = base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| base.strip_prefix("http://").map(|rest| format!("ws://{rest}")))
        .unwrap_or_else(|| base.to_string());
    format!("{ws}/socket?api_key={token}&deviceId={}", jellyfin::device_id())
}

/// The session's WebSocket, kept open (reconnecting after a drop) for as
/// long as the subscription runs. Answers the server's keep-alive
/// requests as jellyfin-web does: a `KeepAlive` every half of the
/// interval the server asks for.
pub fn socket(base: &str, token: &str) -> impl cosmic::iced::futures::Stream<Item = Inbound> {
    use cosmic::iced::futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Ws;

    let url = socket_url(base, token);
    cosmic::iced::stream::channel(64, move |mut out: cosmic::iced::futures::channel::mpsc::Sender<Inbound>| async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            match tokio_tungstenite::connect_async(url.as_str()).await {
                Ok((ws, _)) => {
                    log::info!("SyncPlay: WebSocket open");
                    backoff = Duration::from_secs(1);
                    let _ = out.send(Inbound::Connected).await;
                    let (mut tx, mut rx) = ws.split();
                    let mut keep_alive = tokio::time::interval(Duration::from_secs(30));
                    keep_alive.reset();
                    loop {
                        tokio::select! {
                            msg = rx.next() => match msg {
                                Some(Ok(Ws::Text(text))) => {
                                    if let Some(secs) = force_keep_alive(&text) {
                                        keep_alive = tokio::time::interval(Duration::from_secs_f64((secs / 2.0).max(1.0)));
                                        keep_alive.reset();
                                    } else if let Some(event) = parse_message(&text) {
                                        if out.send(event).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                Some(Ok(Ws::Ping(data))) => {
                                    let _ = tx.send(Ws::Pong(data)).await;
                                }
                                Some(Ok(Ws::Close(_))) | None => break,
                                Some(Ok(_)) => {}
                                Some(Err(e)) => {
                                    log::warn!("SyncPlay: WebSocket: {e}");
                                    break;
                                }
                            },
                            _ = keep_alive.tick() => {
                                if tx.send(Ws::text(r#"{"MessageType":"KeepAlive"}"#)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    let _ = out.send(Inbound::Disconnected).await;
                }
                Err(e) => log::info!("SyncPlay: can't open the WebSocket: {e}"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    })
}

/// The interval (seconds) in a `ForceKeepAlive` message.
fn force_keep_alive(text: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    (v.get("MessageType")?.as_str()? == "ForceKeepAlive").then(|| v.get("Data").and_then(|d| d.as_f64()).unwrap_or(60.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_times_round_trip() {
        let ms = parse_time("2026-10-22T20:01:46.1234567Z").unwrap();
        assert_eq!(format_time(ms), "2026-10-22T20:01:46.123Z");
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_time("2000-03-01T00:00:01.5"), Some(951_868_801_500.0));
        assert_eq!(format_time(951_868_801_500.0), "2000-03-01T00:00:01.500Z");
    }

    #[test]
    fn time_sync_trusts_the_shortest_round_trip() {
        let mut ts = TimeSync::default();
        // Server 500 ms ahead; 100 ms round trip.
        ts.add(Measurement { request_sent: 0.0, request_received: 550.0, response_sent: 550.0, response_received: 100.0 });
        // A slow, lopsided round trip that would say 400 ms.
        ts.add(Measurement { request_sent: 0.0, request_received: 900.0, response_sent: 900.0, response_received: 1000.0 });
        assert_eq!(ts.offset(), 500.0);
        assert_eq!(ts.ping(), 50.0);
        assert_eq!(ts.to_local(1500.0), 1000.0);
        assert_eq!(ts.next_interval(), PING_GREEDY);
    }

    #[test]
    fn socket_messages_parse() {
        let cmd = r#"{"MessageType":"SyncPlayCommand","MessageId":"x","Data":{"GroupId":"g","PlaylistItemId":"p1","When":"2026-10-22T20:00:00.5Z","PositionTicks":1200000000,"Command":"Unpause","EmittedAt":"2026-10-22T20:00:00Z"}}"#;
        let Some(Inbound::Command(c)) = parse_message(cmd) else { panic!("not a command") };
        assert_eq!(c.kind, CommandKind::Unpause);
        assert_eq!(c.when - c.emitted_at, 500.0);
        assert_eq!(c.position_ticks, Some(1_200_000_000));

        let joined = r#"{"MessageType":"SyncPlayGroupUpdate","Data":{"GroupId":"g","Type":"UserJoined","Data":"tim"}}"#;
        assert_eq!(parse_message(joined), Some(Inbound::Group(GroupUpdate::UserJoined("tim".into()))));

        let queue = r#"{"MessageType":"SyncPlayGroupUpdate","Data":{"GroupId":"g","Type":"PlayQueue","Data":{"Reason":"NewPlaylist","LastUpdate":"2026-10-22T20:00:00Z","Playlist":[{"ItemId":"a","PlaylistItemId":"p1"}],"PlayingItemIndex":0,"StartPositionTicks":0,"IsPlaying":true,"ShuffleMode":"Sorted","RepeatMode":"RepeatNone"}}}"#;
        let Some(Inbound::Group(GroupUpdate::PlayQueue(q))) = parse_message(queue) else { panic!("not a queue") };
        assert_eq!(q.current().map(|i| i.item_id.as_str()), Some("a"));

        assert_eq!(force_keep_alive(r#"{"MessageType":"ForceKeepAlive","Data":60}"#), Some(60.0));
        assert_eq!(parse_message(r#"{"MessageType":"KeepAlive"}"#), None);
    }

    #[test]
    fn ticks() {
        assert_eq!(seconds_to_ticks(1.5), 15_000_000);
        assert_eq!(ticks_to_seconds(15_000_000), 1.5);
    }
}
