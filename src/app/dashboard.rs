//! The Dashboard, for administrators: Scan All Libraries, the active
//! sessions (who's watching what, where and how), and the scheduled tasks
//! with Run and Stop. Only the requests in `crate::admin` are made; Scan
//! and Run ask first, as a mistaken click on either starts real work on
//! the server.

use std::collections::BTreeMap;

use cosmic::app::Task;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, icon};
use cosmic::Element;

use super::widgets::{page_scroll, picture, progress, truncate, LANDSCAPE};
use super::{App, Message, LANDSCAPE_W};
use crate::admin::{self, Session};
use crate::images::Key;

/// What's waiting for the user to confirm.
#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    Scan,
    Run { id: String, name: String },
}

#[derive(Clone, Debug)]
pub enum DashMsg {
    Refresh,
    /// Results, tagged with the account they're for.
    Sessions(String, Result<Vec<Session>, String>),
    Tasks(String, Result<Vec<admin::Task>, String>),
    Ask(Confirm),
    Cancel,
    Confirmed,
    Stop(String),
    /// A scan, run or stop went through (or didn't).
    Done(&'static str, Result<(), String>),
}

#[derive(Default)]
pub struct Dashboard {
    pub sessions: Option<Result<Vec<Session>, String>>,
    pub tasks: Option<Result<Vec<admin::Task>, String>>,
    pub confirm: Option<Confirm>,
    /// What the last action did ("Library scan started.") or why it failed.
    pub message: Option<String>,
}

impl Dashboard {
    /// Whether a task is running, so the page refreshes more often.
    pub fn busy(&self) -> bool {
        matches!(&self.tasks, Some(Ok(tasks)) if tasks.iter().any(|t| t.running()))
    }
}

/// Thumbnail size for what a session is playing.
const THUMB: (f32, f32) = (LANDSCAPE.0 * 0.6, LANDSCAPE.1 * 0.6);

impl App {
    /// Reloads the sessions and tasks.
    pub(super) fn refresh_dashboard(&mut self) -> Task<Message> {
        let Some(account) = self.account.clone().filter(|_| self.admin) else { return Task::none() };
        let key = account.key();
        let (a, k) = (account.clone(), key.clone());
        let sessions = cosmic::task::future(async move {
            Message::Dashboard(DashMsg::Sessions(k, admin::sessions(&a).await))
        });
        let tasks = cosmic::task::future(async move {
            Message::Dashboard(DashMsg::Tasks(key, admin::tasks(&account).await))
        });
        Task::batch([sessions, tasks])
    }

    pub(super) fn update_dashboard(&mut self, msg: DashMsg) -> Task<Message> {
        let current = self.account.as_ref().map(|a| a.key());
        match msg {
            DashMsg::Refresh => return self.refresh_dashboard(),
            DashMsg::Sessions(key, result) => {
                if Some(&key) != current.as_ref() {
                    return Task::none();
                }
                let keys: Vec<Key> = match &result {
                    Ok(sessions) => sessions
                        .iter()
                        .filter_map(|s| s.now_playing_item.as_ref()?.landscape())
                        .map(|img| self.key(img, LANDSCAPE_W))
                        .collect(),
                    Err(_) => Vec::new(),
                };
                self.dashboard.sessions = Some(result);
                return self.fetch_images(keys);
            }
            DashMsg::Tasks(key, result) => {
                if Some(&key) == current.as_ref() {
                    self.dashboard.tasks = Some(result);
                }
            }
            DashMsg::Ask(what) => {
                self.dashboard.confirm = Some(what);
                self.dashboard.message = None;
            }
            DashMsg::Cancel => self.dashboard.confirm = None,
            DashMsg::Confirmed => {
                let Some(account) = self.account.clone() else { return Task::none() };
                return match self.dashboard.confirm.take() {
                    Some(Confirm::Scan) => cosmic::task::future(async move {
                        Message::Dashboard(DashMsg::Done("Library scan started.", admin::scan_libraries(&account).await))
                    }),
                    Some(Confirm::Run { id, .. }) => cosmic::task::future(async move {
                        Message::Dashboard(DashMsg::Done("Task started.", admin::start_task(&account, &id).await))
                    }),
                    None => Task::none(),
                };
            }
            DashMsg::Stop(id) => {
                let Some(account) = self.account.clone() else { return Task::none() };
                return cosmic::task::future(async move {
                    Message::Dashboard(DashMsg::Done("Stopping the task…", admin::stop_task(&account, &id).await))
                });
            }
            DashMsg::Done(what, result) => {
                self.dashboard.message = Some(match result {
                    Ok(()) => what.to_string(),
                    Err(e) => e,
                });
                return self.refresh_dashboard();
            }
        }
        Task::none()
    }

    pub(super) fn view_dashboard(&self) -> Element<'_, Message> {
        let d = &self.dashboard;
        let msg = |m: DashMsg| Message::Dashboard(m);
        let server = self.account.as_ref().map(|a| a.server_name.clone()).unwrap_or_default();

        // What the last action did sits beside the button, so nothing on
        // the page moves under the pointer when it appears.
        let status = d.message.clone().unwrap_or_default();
        let title = widget::row::with_capacity(3)
            .push(
                widget::column::with_capacity(2)
                    .push(widget::text::title3("Dashboard"))
                    .push(widget::text::caption(server))
                    .width(Length::Fill),
            )
            .push(widget::text::body(status))
            .push(
                widget::button::standard("Scan All Libraries")
                    .leading_icon(icon::from_name("view-refresh-symbolic"))
                    .on_press(msg(DashMsg::Ask(Confirm::Scan))),
            )
            .spacing(16)
            .align_y(Alignment::Center);

        let mut col = widget::column::with_capacity(8).spacing(24).padding([20, 28, 28, 28]).push(title);

        // Scan asks up here, by its button; a task asks in its own row.
        if d.confirm == Some(Confirm::Scan) {
            let (question, detail, action) = (
                "Scan all libraries now?",
                "The server looks for new, changed and removed files in every library, as the dashboard's Scan All Libraries does.",
                "Scan",
            );
            col = col.push(
                widget::container(
                    widget::row::with_capacity(4)
                        .push(
                            widget::column::with_capacity(2)
                                .push(widget::text::heading(question))
                                .push(widget::text::caption(detail))
                                .spacing(4)
                                .width(Length::Fill),
                        )
                        .push(widget::button::standard("Cancel").on_press(msg(DashMsg::Cancel)))
                        .push(widget::button::suggested(action).on_press(msg(DashMsg::Confirmed)))
                        .spacing(12)
                        .align_y(Alignment::Center),
                )
                .padding(16)
                .class(cosmic::theme::Container::Card),
            );
        }

        col = col.push(self.view_sessions());
        col = col.push(self.view_tasks());
        page_scroll(col)
    }

    fn view_sessions(&self) -> Element<'_, Message> {
        let mut col = widget::column::with_capacity(8).spacing(12).push(widget::text::title4("Active Devices"));
        match &self.dashboard.sessions {
            None => col = col.push(widget::text::body("Loading…")),
            Some(Err(e)) => col = col.push(widget::text::body(format!("Couldn't load the sessions: {e}"))),
            Some(Ok(sessions)) if sessions.is_empty() => col = col.push(widget::text::body("No active devices.")),
            Some(Ok(sessions)) => {
                // Who's watching first, then the rest by most recent.
                let mut sorted: Vec<&Session> = sessions.iter().collect();
                sorted.sort_by(|a, b| {
                    b.now_playing_item.is_some().cmp(&a.now_playing_item.is_some()).then(b.last_activity_date.cmp(&a.last_activity_date))
                });
                for s in sorted {
                    col = col.push(self.session_card(s));
                }
            }
        }
        col.into()
    }

    fn session_card<'a>(&'a self, s: &'a Session) -> Element<'a, Message> {
        let (w, h) = THUMB;
        let user = s.user_name.clone().unwrap_or_else(|| "(no user)".into());
        let mut info = widget::column::with_capacity(6)
            .spacing(3)
            .width(Length::Fill)
            .push(widget::text::heading(user))
            .push(widget::text::caption(format!("{} {}  ·  {}", s.client, s.application_version, s.device_name)));

        let thumb = match &s.now_playing_item {
            Some(item) => {
                let title = if item.is_episode() {
                    let code = item.episode_code().map(|c| format!(" {c}")).unwrap_or_default();
                    format!("{}{code} · {}", item.series_name.clone().unwrap_or_default(), item.name)
                } else {
                    match item.production_year {
                        Some(y) => format!("{} ({y})", item.name),
                        None => item.name.clone(),
                    }
                };
                info = info.push(widget::text::body(truncate(&title, 70)));
                let position = s.play_state.position_ticks.unwrap_or(0) as f64 / 10_000_000.0;
                let duration = item.run_time_ticks.unwrap_or(0) as f64 / 10_000_000.0;
                let mut time = super::player::clock(position);
                if duration > 0.0 {
                    time = format!("{time} / {}", super::player::clock(duration));
                }
                if s.play_state.is_paused {
                    time.push_str("  ·  Paused");
                }
                let fraction = if duration > 0.0 { (position / duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
                info = info.push(
                    widget::row::with_capacity(2)
                        .push(progress(fraction, 220.0))
                        .push(widget::text::caption(time))
                        .spacing(10)
                        .align_y(Alignment::Center),
                );
                if let Some(method) = s.method() {
                    info = info.push(widget::text::caption(method));
                }
                if let Some(t) = &s.transcoding_info {
                    let reasons = t.reasons();
                    if !reasons.is_empty() {
                        info = info.push(widget::text::caption(reasons));
                    }
                }
                let key = item.landscape().map(|img| self.key(img, LANDSCAPE_W));
                picture(key.as_ref().and_then(|k| self.image(k)), w, h, "video-x-generic-symbolic")
            }
            None => {
                info = info.push(widget::text::caption(format!("Last active {}", admin::ago(&s.last_activity_date))));
                widget::container(icon::from_name(device_icon(&s.client)).size(40))
                    .center(Length::Fixed(h))
                    .width(Length::Fixed(w))
                    .into()
            }
        };
        if let Some(ip) = &s.remote_end_point {
            info = info.push(widget::text::caption(ip.clone()));
        }
        widget::container(widget::row::with_capacity(2).push(thumb).push(info).spacing(16).align_y(Alignment::Center))
            .padding(12)
            .width(Length::Fill)
            .class(cosmic::theme::Container::Card)
            .into()
    }

    fn view_tasks(&self) -> Element<'_, Message> {
        let msg = |m: DashMsg| Message::Dashboard(m);
        let mut col = widget::column::with_capacity(8).spacing(12).push(widget::text::title4("Scheduled Tasks"));
        let tasks = match &self.dashboard.tasks {
            None => return col.push(widget::text::body("Loading…")).into(),
            Some(Err(e)) => return col.push(widget::text::body(format!("Couldn't load the tasks: {e}"))).into(),
            Some(Ok(tasks)) => tasks,
        };
        let mut by_category: BTreeMap<&str, Vec<&admin::Task>> = BTreeMap::new();
        for t in tasks {
            by_category.entry(t.category.as_str()).or_default().push(t);
        }
        for (category, tasks) in by_category {
            let mut list = widget::list_column();
            for t in tasks {
                let mut text = widget::column::with_capacity(3).spacing(2).width(Length::Fill).push(widget::text::body(t.name.clone()));
                if !t.description.is_empty() {
                    text = text.push(widget::text::caption(truncate(&t.description, 110)));
                }
                if let Some(r) = &t.last_execution_result {
                    let mut last = format!("{} {}", r.status, admin::ago(&r.end_time_utc));
                    if let Some(e) = r.error_message.as_ref().filter(|e| !e.is_empty()) {
                        last.push_str(&format!(": {}", truncate(e, 80)));
                    }
                    text = text.push(widget::text::caption(last));
                }
                let asking = matches!(&self.dashboard.confirm, Some(Confirm::Run { id, .. }) if *id == t.id);
                let control: Element<'_, Message> = if asking {
                    widget::row::with_capacity(3)
                        .push(widget::text::body("Run it now?"))
                        .push(widget::button::standard("Cancel").on_press(msg(DashMsg::Cancel)))
                        .push(widget::button::suggested("Run").on_press(msg(DashMsg::Confirmed)))
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .into()
                } else if t.running() {
                    let pct = t.current_progress_percentage.unwrap_or(0.0);
                    let label = if t.state == "Cancelling" { "Stopping…".to_string() } else { format!("{pct:.0}%") };
                    widget::row::with_capacity(3)
                        .push(progress((pct / 100.0) as f32, 120.0))
                        .push(widget::text::caption(label))
                        .push(widget::button::standard("Stop").on_press_maybe((t.state == "Running").then(|| msg(DashMsg::Stop(t.id.clone())))))
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .into()
                } else {
                    widget::button::standard("Run")
                        .leading_icon(icon::from_name("media-playback-start-symbolic"))
                        .on_press(msg(DashMsg::Ask(Confirm::Run { id: t.id.clone(), name: t.name.clone() })))
                        .into()
                };
                list = list.add(widget::row::with_capacity(2).push(text).push(control).spacing(16).align_y(Alignment::Center));
            }
            col = col.push(widget::text::heading(category.to_string())).push(list);
        }
        col.into()
    }
}

/// An icon for a client with nothing playing.
fn device_icon(client: &str) -> &'static str {
    let c = client.to_lowercase();
    if c.contains("android") || c.contains("ios") || c.contains("mobile") {
        "phone-symbolic"
    } else if c.contains("tv") || c.contains("roku") || c.contains("kodi") {
        "video-display-symbolic"
    } else if c.contains("web") {
        "web-browser-symbolic"
    } else {
        "computer-symbolic"
    }
}
