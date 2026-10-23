//! The few administrator tools the app has: the dashboard's active
//! sessions, Scan All Libraries, and the scheduled tasks list with Run and
//! Stop. These are the only admin requests the app makes:
//!
//! - `GET /Users/Me` (is this user an administrator?)
//! - `GET /Sessions` (read only)
//! - `GET /ScheduledTasks` (read only)
//! - `POST /Library/Refresh` (the dashboard's Scan All Libraries)
//! - `POST` / `DELETE /ScheduledTasks/Running/{id}` (a task's Run / Stop)
//!
//! Nothing here changes settings, users, libraries or schedules. The server
//! refuses the last three for anyone but an administrator, and the app
//! only shows them to one.

use serde::Deserialize;

use crate::jellyfin::{self, Account, Item};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Me {
    policy: Policy,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Policy {
    is_administrator: bool,
}

pub async fn is_admin(account: &Account) -> Result<bool, String> {
    let me: Me = jellyfin::get_json(&format!("{}/Users/Me", account.url), &account.auth()).await?;
    Ok(me.policy.is_administrator)
}

// ── Sessions ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Session {
    pub id: String,
    pub user_name: Option<String>,
    pub client: String,
    pub application_version: String,
    pub device_name: String,
    pub remote_end_point: Option<String>,
    pub last_activity_date: String,
    pub now_playing_item: Option<Item>,
    pub play_state: PlayState,
    pub transcoding_info: Option<TranscodingInfo>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlayState {
    pub position_ticks: Option<i64>,
    pub is_paused: bool,
    pub play_method: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TranscodingInfo {
    pub is_video_direct: bool,
    pub is_audio_direct: bool,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub completion_percentage: Option<f64>,
    /// A list of reasons on current servers, a comma-separated string on
    /// some older ones.
    pub transcode_reasons: serde_json::Value,
}

impl TranscodingInfo {
    /// "Container not supported, Audio codec not supported".
    pub fn reasons(&self) -> String {
        let raw: Vec<String> = match &self.transcode_reasons {
            serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
            serde_json::Value::String(s) => s.split(',').map(|r| r.trim().to_string()).collect(),
            _ => Vec::new(),
        };
        raw.iter().filter(|r| !r.is_empty()).map(|r| spaced(r)).collect::<Vec<_>>().join(", ")
    }
}

/// "ContainerNotSupported" → "Container not supported".
fn spaced(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

impl Session {
    /// "Direct Play", "Direct Stream" or "Transcode (H264, AAC)".
    pub fn method(&self) -> Option<String> {
        let method = self.play_state.play_method.as_deref()?;
        Some(match (method, &self.transcoding_info) {
            ("Transcode", Some(t)) => {
                let codecs: Vec<String> = [
                    (!t.is_video_direct).then(|| t.video_codec.clone()).flatten(),
                    (!t.is_audio_direct).then(|| t.audio_codec.clone()).flatten(),
                ]
                .into_iter()
                .flatten()
                .map(|c| c.to_uppercase())
                .collect();
                if codecs.is_empty() {
                    "Transcode".into()
                } else {
                    format!("Transcode ({})", codecs.join(", "))
                }
            }
            ("DirectPlay", _) => "Direct Play".into(),
            ("DirectStream", _) => "Direct Stream".into(),
            (other, _) => spaced(other),
        })
    }
}

/// The sessions active in the last 16 minutes, as the dashboard lists them.
pub async fn sessions(account: &Account) -> Result<Vec<Session>, String> {
    jellyfin::get_json(&format!("{}/Sessions?activeWithinSeconds=960", account.url), &account.auth()).await
}

// ── Scheduled tasks ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    /// Idle, Running or Cancelling.
    pub state: String,
    pub current_progress_percentage: Option<f64>,
    pub last_execution_result: Option<TaskResult>,
    pub is_hidden: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TaskResult {
    pub end_time_utc: String,
    /// Completed, Failed, Cancelled or Aborted.
    pub status: String,
    pub error_message: Option<String>,
}

impl Task {
    pub fn running(&self) -> bool {
        self.state != "Idle"
    }
}

pub async fn tasks(account: &Account) -> Result<Vec<Task>, String> {
    let mut tasks: Vec<Task> =
        jellyfin::get_json(&format!("{}/ScheduledTasks?isHidden=false", account.url), &account.auth()).await?;
    tasks.retain(|t| !t.is_hidden);
    tasks.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
    Ok(tasks)
}

fn refused(e: String) -> String {
    if e.contains("403") {
        "The server refused: this needs an administrator account.".into()
    } else {
        e
    }
}

pub async fn start_task(account: &Account, id: &str) -> Result<(), String> {
    let req = jellyfin::client().post(format!("{}/ScheduledTasks/Running/{id}", account.url));
    jellyfin::send(req, &account.auth()).await.map_err(refused)
}

pub async fn stop_task(account: &Account, id: &str) -> Result<(), String> {
    let req = jellyfin::client().delete(format!("{}/ScheduledTasks/Running/{id}", account.url));
    jellyfin::send(req, &account.auth()).await.map_err(refused)
}

/// The dashboard's Scan All Libraries.
pub async fn scan_libraries(account: &Account) -> Result<(), String> {
    let req = jellyfin::client().post(format!("{}/Library/Refresh", account.url));
    jellyfin::send(req, &account.auth()).await.map_err(refused)
}

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago", from a server
/// time.
pub fn ago(server_time: &str) -> String {
    let Some(then) = crate::syncplay::parse_time(server_time) else { return String::new() };
    let secs = ((crate::syncplay::now_ms() - then) / 1000.0).max(0.0) as u64;
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        _ => plural(secs / 86_400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_describe_their_playback() {
        let json = r#"{"Id":"s","UserName":"anna","Client":"Jellyfin Android TV","DeviceName":"Living Room",
            "NowPlayingItem":{"Id":"m","Name":"Film","Type":"Movie","RunTimeTicks":100},
            "PlayState":{"PositionTicks":50,"IsPaused":true,"PlayMethod":"Transcode"},
            "TranscodingInfo":{"IsVideoDirect":false,"IsAudioDirect":true,"VideoCodec":"h264","AudioCodec":"aac",
            "TranscodeReasons":["ContainerNotSupported","VideoCodecNotSupported"]}}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert_eq!(s.method().as_deref(), Some("Transcode (H264)"));
        assert_eq!(s.transcoding_info.unwrap().reasons(), "Container not supported, Video codec not supported");
        assert!(s.play_state.is_paused);
        let old: TranscodingInfo = serde_json::from_str(r#"{"TranscodeReasons":"ContainerBitrateExceedsLimit"}"#).unwrap();
        assert_eq!(old.reasons(), "Container bitrate exceeds limit");
    }

    #[test]
    fn tasks_parse() {
        let json = r#"[{"Id":"t1","Name":"Scan Media Library","Category":"Library","State":"Running",
            "CurrentProgressPercentage":42.5,"IsHidden":false},
            {"Id":"t2","Name":"Clean Cache Directory","Category":"Maintenance","State":"Idle",
            "LastExecutionResult":{"EndTimeUtc":"2026-10-22T20:00:00Z","Status":"Completed"}}]"#;
        let tasks: Vec<Task> = serde_json::from_str(json).unwrap();
        assert!(tasks[0].running() && !tasks[1].running());
        assert_eq!(tasks[1].last_execution_result.as_ref().unwrap().status, "Completed");
    }
}
