//! SyncPlay, the player's side: jellyfin-web's SyncPlay Manager,
//! PlaybackCore, QueueCore and Controller (`plugins/syncPlay/core`), on
//! mpv. The protocol itself is in `crate::syncplay`.
//!
//! In a group, nothing the user does to playback happens straight away:
//! play, pause, seek, next and previous become requests to the server,
//! which answers every member with a command to carry out at a set server
//! time. Each member loads the group's current item paused, reports Ready,
//! and the server starts everyone together. A member that stalls for 3 s
//! reports Buffering, and the group waits for it. Sync correction (off by
//! default, as in jellyfin-web) nudges the speed or seeks when a member
//! drifts.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cosmic::app::Task;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::Element;

use super::{App, Message};
use crate::jellyfin::{self, Item};
use crate::syncplay::{self as sp, Command, CommandKind, GroupInfo, GroupUpdate, Inbound, PlayQueue, TICKS_PER_MS};

/// jellyfin-web's sync settings (PlaybackCore.loadPreferences defaults).
const MIN_DELAY_SPEED_TO_SYNC: f64 = 60.0;
const MAX_DELAY_SPEED_TO_SYNC: f64 = 3000.0;
const SPEED_TO_SYNC_DURATION: f64 = 1000.0;
const MIN_DELAY_SKIP_TO_SYNC: f64 = 400.0;
/// A stall shorter than this isn't reported (HtmlVideoPlayer's
/// minBufferingThresholdMillis).
const BUFFERING_THRESHOLD: Duration = Duration::from_millis(3000);
/// How long to wait for the player to start or finish a seek
/// (Helper.WaitForEventDefaultTimeout).
const WAIT_FOR_PLAYER: Duration = Duration::from_secs(30);
/// How long a notice stays up.
const NOTICE_FOR: Duration = Duration::from_secs(4);

/// What a scheduled command does when its time comes.
#[derive(Clone, Debug)]
pub enum Timed {
    Unpause,
    Pause(i64),
    Stop,
    Seek(i64),
    /// Sync correction may act again.
    EnableSync,
    /// SpeedToSync's time is up: back to normal speed.
    EndSpeed,
}

#[derive(Clone, Debug)]
pub enum SyncMsg {
    Socket(Inbound),
    Measured(Result<sp::Measurement, String>),
    PingDue,
    Groups(Result<Vec<GroupInfo>, String>),
    ToggleMenu,
    NewGroup,
    Join(String),
    Leave,
    /// "Stop local playback": leave the group's playback, stay in it.
    Halt,
    /// "Resume local playback".
    Resume,
    /// A scheduled command; dropped if anything was scheduled since.
    Fire(u64, Timed),
    Sent(Result<(), String>),
    /// A play request turned into the group's new queue.
    QueueBuilt(Result<(Vec<String>, i64), String>),
    SetCorrection(bool),
    SetSpeedToSync(bool),
    SetSkipToSync(bool),
    NoticeTick,
}

#[derive(Default)]
pub struct SyncState {
    pub group: Option<GroupInfo>,
    /// Idle, Waiting, Paused or Playing.
    pub group_state: String,
    /// The group's LastUpdatedAt when joined (server ms); `None` when not
    /// in a group. Commands from before it are stale.
    enabled_at: Option<f64>,
    /// Time synced since joining; commands wait for it.
    ready: bool,
    queued: Option<Command>,
    /// The group's latest command (Manager.lastPlaybackCommand).
    last_command: Option<Command>,
    /// The latest one acted on (PlaybackCore.lastCommand).
    applied: Option<Command>,
    pub queue: Option<PlayQueue>,
    /// False after "Stop local playback".
    following: bool,
    time: sp::TimeSync,
    pinging: bool,
    /// Bumped to drop whatever is scheduled (clearScheduledCommand).
    generation: u64,
    /// Sync correction may act.
    sync_enabled: bool,
    last_sync: Option<Instant>,
    attempts: u32,
    /// How far behind the group this member is, in ms.
    pub diff_ms: f64,
    /// The correction going on, for the player's corner.
    pub method: Option<String>,
    speed_changed: bool,
    told_buffering: bool,
    stall_since: Option<Instant>,
    /// Waiting for the group's item to load: its PlaylistItemId and
    /// when to give up.
    await_start: Option<(String, Instant)>,
    /// Waiting for a group seek to land: where, and since when.
    await_seek: Option<(f64, Instant)>,
    /// What the app last set mpv's pause to, to tell the user pausing in
    /// mpv's own window.
    set_paused: Option<bool>,
    seen_paused: Option<bool>,
    /// The groups to join, while the menu is open.
    pub groups: Option<Result<Vec<GroupInfo>, String>>,
    /// The context drawer shows the SyncPlay menu (not Settings).
    pub menu: bool,
    pub notices: VecDeque<(String, Instant)>,
    pub connected: bool,
}

impl SyncState {
    pub fn new() -> Self {
        SyncState { following: true, ..Default::default() }
    }
}

impl App {
    /// In a SyncPlay group.
    pub(super) fn sync_active(&self) -> bool {
        self.sync.enabled_at.is_some()
    }

    /// Playing the group's item, so the player's buttons go to the group.
    pub(super) fn sync_playing(&self) -> bool {
        self.sync_active() && self.playback.as_ref().is_some_and(|pb| pb.sync_pid.is_some())
    }

    fn notice(&mut self, text: impl Into<String>) {
        self.sync.notices.push_back((text.into(), Instant::now()));
        while self.sync.notices.len() > 3 {
            self.sync.notices.pop_front();
        }
    }

    fn sent<F>(&self, request: impl FnOnce(jellyfin::Account) -> F) -> Task<Message>
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let Some(account) = self.account.clone() else { return Task::none() };
        let fut = request(account);
        cosmic::task::future(async move { Message::Sync(SyncMsg::Sent(fut.await)) })
    }

    fn now_server(&self) -> f64 {
        self.sync.time.to_server(sp::now_ms())
    }

    /// Where the group is now, from a position at a server time.
    fn estimate_ticks(&self, ticks: i64, when: f64) -> i64 {
        ticks + ((self.now_server() - when) * TICKS_PER_MS) as i64
    }

    fn current_pid(&self) -> Option<String> {
        self.sync.queue.as_ref().and_then(|q| q.current()).map(|i| i.playlist_item_id.clone())
    }

    /// The player has the group's item loaded (isPlaybackActive).
    fn playback_active(&self) -> bool {
        let player_up = self.player.as_ref().is_some_and(|p| !p.state().closed);
        player_up && self.playback.as_ref().is_some_and(|pb| pb.started && pb.sync_pid.is_some())
    }

    fn position_ticks(&self) -> i64 {
        self.player.as_ref().map_or(0, |p| sp::seconds_to_ticks(p.state().position))
    }

    fn local_pause(&mut self) {
        if let Some(p) = &self.player {
            p.set_paused(true);
            self.sync.set_paused = Some(true);
        }
    }

    fn local_unpause(&mut self) {
        if let Some(p) = &self.player {
            p.set_paused(false);
            self.sync.set_paused = Some(false);
        }
    }

    fn local_seek(&self, ticks: i64) {
        if let Some(p) = &self.player {
            p.seek(sp::ticks_to_seconds(ticks).max(0.0));
        }
    }

    /// Where this member is, for Ready and Buffering.
    fn report_position(&mut self, ready: bool) -> Task<Message> {
        let Some(pid) = self.current_pid() else { return Task::none() };
        let state = self.player.as_ref().map(|p| p.state()).unwrap_or_default();
        // mpv takes a moment to apply a pause; what the app asked for counts.
        let paused = self.sync.set_paused.unwrap_or(state.paused);
        let position = sp::Position {
            when: self.now_server(),
            position_ticks: sp::seconds_to_ticks(state.position),
            is_playing: !paused,
            playlist_item_id: pid,
        };
        self.sent(move |a| async move { sp::report(&a, ready, position).await })
    }

    fn schedule(&self, at_local_ms: f64, what: Timed) -> Task<Message> {
        let generation = self.sync.generation;
        let wait = Duration::from_secs_f64(((at_local_ms - sp::now_ms()) / 1000.0).max(0.0));
        cosmic::task::future(async move {
            tokio::time::sleep(wait).await;
            Message::Sync(SyncMsg::Fire(generation, what))
        })
    }

    fn clear_scheduled(&mut self) {
        self.sync.generation += 1;
        self.sync.sync_enabled = false;
        self.end_speed();
        self.sync.method = None;
    }

    fn end_speed(&mut self) {
        if self.sync.speed_changed {
            self.sync.speed_changed = false;
            if let Some(p) = &self.player {
                p.set_speed(1.0);
            }
        }
    }

    // ── Joining and leaving (Manager) ────────────────────────────────────────

    fn enable(&mut self, info: GroupInfo, show: bool) -> Task<Message> {
        let mut show = show;
        if let Some(group) = &self.sync.group {
            if self.sync_active() {
                if group.group_id == info.group_id {
                    return Task::none();
                }
                self.disable(false);
                show = false;
            }
        }
        self.sync.enabled_at = Some(sp::parse_time(&info.last_updated_at).unwrap_or(0.0));
        self.sync.group = Some(info);
        self.sync.ready = false;
        self.sync.following = true;
        self.sync.time.force_update();
        let ping = self.start_pinging();
        if show {
            self.notice("SyncPlay enabled.");
        }
        ping
    }

    fn disable(&mut self, show: bool) {
        self.clear_scheduled();
        self.sync.enabled_at = None;
        self.sync.group = None;
        self.sync.group_state.clear();
        self.sync.ready = false;
        self.sync.following = true;
        self.sync.last_command = None;
        self.sync.applied = None;
        self.sync.queued = None;
        self.sync.queue = None;
        self.sync.await_start = None;
        self.sync.await_seek = None;
        self.sync.told_buffering = false;
        if let Some(pb) = &mut self.playback {
            pb.sync_pid = None;
        }
        if show {
            self.notice("SyncPlay disabled.");
        }
    }

    fn start_pinging(&mut self) -> Task<Message> {
        if self.sync.pinging {
            return Task::none();
        }
        self.sync.pinging = true;
        let Some(account) = self.account.clone() else { return Task::none() };
        cosmic::task::future(async move { Message::Sync(SyncMsg::Measured(sp::measure(&account).await)) })
    }

    fn halt(&mut self) -> Task<Message> {
        self.sync.following = false;
        let ignore = self.sent(|a| async move { sp::set_ignore_wait(&a, true).await });
        let stop = if self.playback_active() { self.stop_playback() } else { Task::none() };
        Task::batch([ignore, stop])
    }

    // ── Commands (Manager.processCommand, PlaybackCore) ──────────────────────

    fn process_command(&mut self, cmd: Command) -> Task<Message> {
        let Some(enabled_at) = self.sync.enabled_at else { return Task::none() };
        if cmd.emitted_at < enabled_at {
            log::debug!("SyncPlay: ignoring an old command");
            return Task::none();
        }
        if !self.sync.ready {
            self.sync.queued = Some(cmd);
            return Task::none();
        }
        self.sync.last_command = Some(cmd.clone());
        if !self.playback_active() {
            return Task::none();
        }
        if Some(&cmd.playlist_item_id) != self.current_pid().as_ref() && cmd.kind != CommandKind::Stop {
            log::warn!("SyncPlay: command for another playlist item");
            return Task::none();
        }
        self.apply_command(cmd)
    }

    fn apply_command(&mut self, cmd: Command) -> Task<Message> {
        let duplicate = self.sync.applied.as_ref().is_some_and(|last| {
            last.when == cmd.when
                && last.position_ticks == cmd.position_ticks
                && last.kind == cmd.kind
                && last.playlist_item_id == cmd.playlist_item_id
        });
        if duplicate {
            // Already scheduled, or done: check playback matches.
            if self.sync.time.to_local(cmd.when) > sp::now_ms() {
                return Task::none();
            }
            let state = self.player.as_ref().map(|p| p.state()).unwrap_or_default();
            let playing = !state.paused;
            let ticks = sp::seconds_to_ticks(state.position);
            let at = cmd.position_ticks.unwrap_or(0);
            return match cmd.kind {
                CommandKind::Unpause if !playing => self.schedule_unpause(cmd.when, at),
                CommandKind::Pause if playing || ticks != at => self.schedule_pause(cmd.when, at),
                CommandKind::Stop if playing => self.schedule_stop(cmd.when),
                CommandKind::Seek if playing || ticks != at => {
                    // Up to ±50 ms off, so the player really seeks.
                    let jitter = ((sp::now_ms() as i64 % 100) - 50) * TICKS_PER_MS as i64;
                    self.schedule_seek(cmd.when, at + jitter)
                }
                CommandKind::Seek => self.report_position(true),
                _ => Task::none(),
            };
        }
        self.sync.applied = Some(cmd.clone());
        let at = cmd.position_ticks.unwrap_or(0);
        log::debug!(
            "SyncPlay will {:?} in {:.0} ms from {:.2} s",
            cmd.kind,
            self.sync.time.to_local(cmd.when) - sp::now_ms(),
            sp::ticks_to_seconds(at)
        );
        match cmd.kind {
            CommandKind::Unpause => self.schedule_unpause(cmd.when, at),
            CommandKind::Pause => self.schedule_pause(cmd.when, at),
            CommandKind::Stop => self.schedule_stop(cmd.when),
            CommandKind::Seek => self.schedule_seek(cmd.when, at),
        }
    }

    fn schedule_unpause(&mut self, when: f64, ticks: i64) -> Task<Message> {
        self.clear_scheduled();
        let now = sp::now_ms();
        let at = self.sync.time.to_local(when);
        if at > now {
            // Seek only if the difference shows.
            if (self.position_ticks() - ticks) as f64 > MIN_DELAY_SKIP_TO_SYNC * TICKS_PER_MS {
                self.local_seek(ticks);
            }
            self.schedule(at, Timed::Unpause)
        } else {
            // The group is already playing: join in where it is.
            let server_ticks = self.estimate_ticks(ticks, when);
            self.local_seek(server_ticks);
            self.local_unpause();
            self.schedule(now + MAX_DELAY_SPEED_TO_SYNC / 2.0, Timed::EnableSync)
        }
    }

    fn schedule_pause(&mut self, when: f64, ticks: i64) -> Task<Message> {
        self.clear_scheduled();
        let at = self.sync.time.to_local(when);
        if at > sp::now_ms() {
            self.schedule(at, Timed::Pause(ticks))
        } else {
            self.local_pause();
            self.local_seek(ticks);
            Task::none()
        }
    }

    fn schedule_stop(&mut self, when: f64) -> Task<Message> {
        self.clear_scheduled();
        let at = self.sync.time.to_local(when);
        if at > sp::now_ms() {
            self.schedule(at, Timed::Stop)
        } else {
            self.stop_playback()
        }
    }

    fn schedule_seek(&mut self, when: f64, ticks: i64) -> Task<Message> {
        self.clear_scheduled();
        let at = self.sync.time.to_local(when);
        if at > sp::now_ms() {
            self.schedule(at, Timed::Seek(ticks))
        } else {
            self.seek_now(ticks);
            Task::none()
        }
    }

    /// A group seek: there paused, then Ready once it has landed (see
    /// `sync_tick`).
    fn seek_now(&mut self, ticks: i64) {
        self.local_pause();
        self.local_seek(ticks);
        self.sync.await_seek = Some((sp::ticks_to_seconds(ticks), Instant::now()));
    }

    fn fire(&mut self, generation: u64, what: Timed) -> Task<Message> {
        if generation != self.sync.generation || !self.sync_active() {
            return Task::none();
        }
        match what {
            Timed::Unpause => {
                self.local_unpause();
                self.schedule(sp::now_ms() + MAX_DELAY_SPEED_TO_SYNC / 2.0, Timed::EnableSync)
            }
            Timed::Pause(ticks) => {
                self.local_pause();
                self.local_seek(ticks);
                Task::none()
            }
            Timed::Stop => self.stop_playback(),
            Timed::Seek(ticks) => {
                self.seek_now(ticks);
                Task::none()
            }
            Timed::EnableSync => {
                self.sync.sync_enabled = true;
                self.sync.method = None;
                Task::none()
            }
            Timed::EndSpeed => {
                self.end_speed();
                self.sync.sync_enabled = true;
                self.sync.method = None;
                Task::none()
            }
        }
    }

    // ── The queue (QueueCore) ────────────────────────────────────────────────

    fn on_play_queue(&mut self, queue: PlayQueue) -> Task<Message> {
        let last = self.sync.queue.as_ref().map_or(0.0, |q| q.last_update_ms());
        if queue.last_update_ms() <= last {
            log::debug!("SyncPlay: ignoring an old queue update");
            return Task::none();
        }
        let old_pid = self.current_pid();
        let reason = queue.reason.clone();
        self.sync.queue = Some(queue);
        match reason.as_str() {
            "NewPlaylist" => {
                if !self.sync.following {
                    self.sync.following = true;
                    let follow = self.sent(|a| async move { sp::set_ignore_wait(&a, false).await });
                    return Task::batch([follow, self.start_group_playback()]);
                }
                self.start_group_playback()
            }
            "SetCurrentItem" | "NextItem" | "PreviousItem" => self.play_current_item(),
            "RemoveItems" if old_pid != self.current_pid() => self.play_current_item(),
            _ => Task::none(),
        }
    }

    /// Loads the group's current item where the group is up to.
    fn start_group_playback(&mut self) -> Task<Message> {
        if !self.sync.following {
            return Task::none();
        }
        let Some(queue) = &self.sync.queue else { return Task::none() };
        if queue.playlist.is_empty() {
            return Task::none();
        }
        let from_command = self.sync.last_command.as_ref().filter(|c| c.emitted_at >= queue.last_update_ms());
        let ticks = match from_command {
            Some(c) => {
                let at = c.position_ticks.unwrap_or(0);
                if c.kind == CommandKind::Unpause {
                    self.estimate_ticks(at, c.when)
                } else {
                    at
                }
            }
            None if queue.is_playing => self.estimate_ticks(queue.start_position_ticks, queue.last_update_ms()),
            None => queue.start_position_ticks,
        };
        self.load_current(ticks)
    }

    /// Plays the item the queue now points at, from the queue's start.
    fn play_current_item(&mut self) -> Task<Message> {
        if !self.sync.following {
            return Task::none();
        }
        let ticks = self.sync.queue.as_ref().map_or(0, |q| q.start_position_ticks);
        self.load_current(ticks)
    }

    fn load_current(&mut self, ticks: i64) -> Task<Message> {
        let Some(current) = self.sync.queue.as_ref().and_then(|q| q.current()).cloned() else { return Task::none() };
        self.clear_scheduled();
        self.sync.applied = None;
        self.sync.await_seek = None;
        self.sync.await_start = Some((current.playlist_item_id.clone(), Instant::now() + WAIT_FOR_PLAYER));
        let item = Item { id: current.item_id, ..Default::default() };
        self.prepare_play(item, false, Some(sp::ticks_to_seconds(ticks).max(0.0)), Some(current.playlist_item_id))
    }

    /// Whether a loaded SyncPlay item is still the one the group wants.
    pub(super) fn sync_wants(&self, pid: Option<&str>) -> bool {
        self.sync_active() && self.sync.following && pid.is_some() && self.current_pid().as_deref() == pid
    }

    /// The group's item couldn't be fetched: as jellyfin-web does when its
    /// player can't start one, stop following the group's playback.
    pub(super) fn sync_load_failed(&mut self) -> Task<Message> {
        if self.sync.await_start.take().is_none() {
            return Task::none();
        }
        self.notice("Failed to enable SyncPlay! Media error.");
        self.halt()
    }

    // ── What the user asks for in a group (Controller) ───────────────────────

    /// Play pressed anywhere: the group gets a new queue, as jellyfin-web
    /// builds it (an episode brings the rest of its show along, when
    /// episodes play on).
    pub(super) fn group_play(&mut self, item: Item, resume: bool) -> Task<Message> {
        let Some(account) = self.account.clone() else { return Task::none() };
        let autoplay = self.settings.autoplay_next;
        cosmic::task::future(async move {
            let result = async {
                let item = match item.kind.as_str() {
                    "Series" => jellyfin::series_start(&account, &item.id)
                        .await?
                        .ok_or_else(|| format!("{} has no episodes.", item.name))?,
                    "Season" => {
                        let series = item.series_id.clone().unwrap_or_default();
                        let eps = jellyfin::episodes(&account, &series, &item.id).await?;
                        eps.iter().find(|e| !e.played()).or(eps.first()).cloned().ok_or_else(|| format!("{} has no episodes.", item.name))?
                    }
                    _ => item,
                };
                let start = if resume { sp::seconds_to_ticks(item.resume_seconds()) } else { 0 };
                let ids = match (&item.series_id, item.is_episode() && autoplay) {
                    (Some(series), true) => jellyfin::episodes_from(&account, series, &item.id)
                        .await
                        .map(|eps| eps.into_iter().map(|e| e.id).collect())
                        .unwrap_or_else(|_| vec![item.id.clone()]),
                    _ => vec![item.id.clone()],
                };
                Ok((ids, start))
            }
            .await;
            Message::Sync(SyncMsg::QueueBuilt(result))
        })
    }

    pub(super) fn group_toggle_pause(&mut self) -> Task<Message> {
        let playing = self.sync.last_command.as_ref().is_some_and(|c| c.kind == CommandKind::Unpause);
        if playing {
            // Paused here at once, for a little control.
            self.local_pause();
            self.sent(|a| async move { sp::pause(&a).await })
        } else {
            self.sent(|a| async move { sp::unpause(&a).await })
        }
    }

    pub(super) fn group_seek(&mut self, seconds: f64) -> Task<Message> {
        let ticks = sp::seconds_to_ticks(seconds.max(0.0));
        self.sent(move |a| async move { sp::seek(&a, ticks).await })
    }

    pub(super) fn group_next(&mut self, forward: bool) -> Task<Message> {
        let Some(pid) = self.current_pid() else { return Task::none() };
        if forward {
            self.sent(move |a| async move { sp::next_item(&a, &pid).await })
        } else {
            self.sent(move |a| async move { sp::previous_item(&a, &pid).await })
        }
    }

    /// The group's queue goes on after this item.
    pub(super) fn group_has_next(&self) -> bool {
        self.sync.queue.as_ref().is_some_and(|q| q.playing_item_index >= 0 && (q.playing_item_index as usize) + 1 < q.playlist.len())
    }

    // ── Following the player (the player wrapper's events) ───────────────────

    /// Called on every player tick while playing the group's item: the
    /// group's item has loaded, a group seek has landed, the player is
    /// stalling or has recovered, and sync correction.
    pub(super) fn sync_tick(&mut self, state: &crate::mpv::State) -> Task<Message> {
        if !self.sync_active() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        let started = self.playback.as_ref().is_some_and(|pb| pb.started && pb.sync_pid.is_some());

        // The item loaded: pause there and tell the group (QueueCore's
        // scheduleReadyRequestOnPlaybackStart).
        if let Some((pid, deadline)) = self.sync.await_start.clone() {
            let ours = self.playback.as_ref().and_then(|pb| pb.sync_pid.as_ref()) == Some(&pid);
            if started && ours {
                self.sync.await_start = None;
                self.local_pause();
                tasks.push(self.report_position(true));
            } else if Instant::now() > deadline {
                self.sync.await_start = None;
                self.notice("Failed to enable SyncPlay! Media error.");
                tasks.push(self.halt());
            }
        }
        if !started {
            return Task::batch(tasks);
        }

        if let Some((target, since)) = self.sync.await_seek {
            let landed = (state.position - target).abs() < 0.5 && !state.buffering;
            if landed && since.elapsed() > Duration::from_millis(150) {
                self.sync.await_seek = None;
                self.local_pause();
                tasks.push(self.report_position(true));
            } else if since.elapsed() > WAIT_FOR_PLAYER {
                self.local_seek(sp::seconds_to_ticks(target));
                self.sync.await_seek = Some((target, Instant::now()));
            }
        }

        // Stalls of 3 s or more hold the group up until they end.
        if state.buffering {
            let since = *self.sync.stall_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= BUFFERING_THRESHOLD && !self.sync.told_buffering {
                self.sync.told_buffering = true;
                tasks.push(self.report_position(false));
            }
        } else {
            self.sync.stall_since = None;
            if self.sync.told_buffering {
                self.sync.told_buffering = false;
                tasks.push(self.report_position(true));
            }
        }

        // Pausing in mpv's own window (its keys don't come through here)
        // asks the group, as the app's own button does.
        let own_window = self.player.as_ref().is_some_and(|p| p.video().is_none());
        let changed = own_window && self.sync.seen_paused.is_some_and(|p| p != state.paused);
        self.sync.seen_paused = Some(state.paused);
        if changed && self.sync.set_paused != Some(state.paused) && self.sync.await_seek.is_none() {
            self.sync.set_paused = Some(state.paused);
            if state.paused {
                tasks.push(self.sent(|a| async move { sp::pause(&a).await }));
            } else {
                // Back to paused until the group says play.
                self.local_pause();
                tasks.push(self.sent(|a| async move { sp::unpause(&a).await }));
            }
        }

        tasks.push(self.correct_sync(state));
        Task::batch(tasks)
    }

    /// PlaybackCore.syncPlaybackTime.
    fn correct_sync(&mut self, state: &crate::mpv::State) -> Task<Message> {
        let Some(cmd) = self.sync.applied.clone() else { return Task::none() };
        if cmd.kind != CommandKind::Unpause || self.sync.told_buffering {
            return Task::none();
        }
        if Some(&cmd.playlist_item_id) != self.current_pid().as_ref() {
            return Task::none();
        }
        // Still waiting for the group's start time.
        if self.sync.time.to_local(cmd.when) > sp::now_ms() {
            return Task::none();
        }
        let current = sp::seconds_to_ticks(state.position);
        let server = self.estimate_ticks(cmd.position_ticks.unwrap_or(0), cmd.when);
        let diff = (server - current) as f64 / TICKS_PER_MS;
        self.sync.diff_ms = diff;

        if self.sync.last_sync.is_some_and(|t| t.elapsed() < Duration::from_secs_f64(MAX_DELAY_SPEED_TO_SYNC / 2000.0)) {
            return Task::none();
        }
        self.sync.last_sync = Some(Instant::now());
        log::debug!("SyncPlay: {diff:.0} ms behind the group");
        if !(self.sync.sync_enabled && self.settings.sync_correction) {
            return Task::none();
        }
        let abs = diff.abs();
        let now = sp::now_ms();
        if self.settings.speed_to_sync && (MIN_DELAY_SPEED_TO_SYNC..MAX_DELAY_SPEED_TO_SYNC).contains(&abs) {
            // Never slower than 0.2× when ahead.
            const MIN_SPEED: f64 = 0.2;
            let mut duration = SPEED_TO_SYNC_DURATION;
            if diff <= -duration * MIN_SPEED {
                duration = abs / (1.0 - MIN_SPEED);
            }
            let speed = 1.0 + diff / duration;
            if let Some(p) = &self.player {
                p.set_speed(speed);
            }
            self.sync.speed_changed = true;
            self.sync.sync_enabled = false;
            self.sync.attempts += 1;
            self.sync.method = Some(format!("SpeedToSync (x{speed:.2})"));
            self.schedule(now + duration, Timed::EndSpeed)
        } else if self.settings.skip_to_sync && abs >= MIN_DELAY_SKIP_TO_SYNC {
            self.local_seek(server);
            self.sync.sync_enabled = false;
            self.sync.attempts += 1;
            self.sync.method = Some(format!("SkipToSync ({})", self.sync.attempts));
            self.schedule(now + MAX_DELAY_SPEED_TO_SYNC / 2.0, Timed::EnableSync)
        } else {
            self.sync.attempts = 0;
            Task::none()
        }
    }

    // ── Messages ─────────────────────────────────────────────────────────────

    pub(super) fn update_sync(&mut self, msg: SyncMsg) -> Task<Message> {
        match msg {
            SyncMsg::Socket(Inbound::Connected) => self.sync.connected = true,
            SyncMsg::Socket(Inbound::Disconnected) => self.sync.connected = false,
            SyncMsg::Socket(Inbound::Command(cmd)) => return self.process_command(cmd),
            SyncMsg::Socket(Inbound::Group(update)) => return self.group_update(update),
            SyncMsg::Measured(result) => {
                match result {
                    Ok(m) => {
                        self.sync.time.add(m);
                        log::debug!("SyncPlay: time offset {:.1} ms, ping {} ms", self.sync.time.offset(), self.sync.time.ping());
                    }
                    Err(e) => log::info!("SyncPlay: time sync: {e}"),
                }
                if !self.sync_active() {
                    self.sync.pinging = false;
                    return Task::none();
                }
                let mut tasks = Vec::new();
                if self.sync.time.ready() {
                    let ping = self.sync.time.ping();
                    tasks.push(self.sent(move |a| async move { sp::ping(&a, ping).await }));
                    if !self.sync.ready {
                        self.sync.ready = true;
                        if let Some(cmd) = self.sync.queued.take() {
                            tasks.push(self.process_command(cmd));
                        }
                    }
                }
                let wait = self.sync.time.next_interval();
                tasks.push(cosmic::task::future(async move {
                    tokio::time::sleep(wait).await;
                    Message::Sync(SyncMsg::PingDue)
                }));
                return Task::batch(tasks);
            }
            SyncMsg::PingDue => {
                self.sync.pinging = false;
                if self.sync_active() {
                    return self.start_pinging();
                }
            }
            SyncMsg::Groups(result) => self.sync.groups = Some(result),
            SyncMsg::ToggleMenu => {
                if self.core.window.show_context && self.sync.menu {
                    self.core.window.show_context = false;
                    return Task::none();
                }
                self.sync.menu = true;
                self.core.window.show_context = true;
                if !self.sync_active() {
                    self.sync.groups = None;
                    let Some(account) = self.account.clone() else { return Task::none() };
                    return cosmic::task::future(async move { Message::Sync(SyncMsg::Groups(sp::list_groups(&account).await)) });
                }
            }
            SyncMsg::NewGroup => {
                self.core.window.show_context = false;
                let name = format!("{}'s group", self.account.as_ref().map(|a| a.user_name.as_str()).unwrap_or("My"));
                return self.sent(move |a| async move { sp::new_group(&a, &name).await });
            }
            SyncMsg::Join(id) => {
                self.core.window.show_context = false;
                return self.sent(move |a| async move { sp::join(&a, &id).await });
            }
            SyncMsg::Leave => {
                self.core.window.show_context = false;
                return self.sent(|a| async move { sp::leave(&a).await });
            }
            SyncMsg::Halt => {
                self.core.window.show_context = false;
                return self.halt();
            }
            SyncMsg::Resume => {
                self.core.window.show_context = false;
                self.sync.following = true;
                let follow = self.sent(|a| async move { sp::set_ignore_wait(&a, false).await });
                return Task::batch([follow, self.start_group_playback()]);
            }
            SyncMsg::Fire(generation, what) => return self.fire(generation, what),
            SyncMsg::Sent(Err(e)) => {
                log::warn!("{e}");
                if e.contains("HTTP 403") || e.contains("not authorized") {
                    self.notice("Permission required to use SyncPlay.");
                }
            }
            SyncMsg::Sent(Ok(())) => {}
            SyncMsg::QueueBuilt(Ok((ids, start))) => {
                return self.sent(move |a| async move { sp::set_new_queue(&a, &ids, 0, start).await });
            }
            SyncMsg::QueueBuilt(Err(e)) => self.error = Some(e),
            SyncMsg::SetCorrection(on) => {
                self.settings.sync_correction = on;
                self.settings.save();
            }
            SyncMsg::SetSpeedToSync(on) => {
                self.settings.speed_to_sync = on;
                self.settings.save();
            }
            SyncMsg::SetSkipToSync(on) => {
                self.settings.skip_to_sync = on;
                self.settings.save();
            }
            SyncMsg::NoticeTick => {
                while self.sync.notices.front().is_some_and(|(_, at)| at.elapsed() >= NOTICE_FOR) {
                    self.sync.notices.pop_front();
                }
            }
        }
        Task::none()
    }

    /// Manager.processGroupUpdate.
    fn group_update(&mut self, update: GroupUpdate) -> Task<Message> {
        match update {
            GroupUpdate::PlayQueue(queue) => return self.on_play_queue(queue),
            GroupUpdate::UserJoined(user) => {
                if let Some(group) = &mut self.sync.group {
                    group.participants.push(user.clone());
                }
                self.notice(format!("{user} has joined the group."));
            }
            GroupUpdate::UserLeft(user) => {
                if let Some(group) = &mut self.sync.group {
                    group.participants.retain(|u| *u != user);
                }
                self.notice(format!("{user} has left the group."));
            }
            GroupUpdate::GroupJoined(info) => return self.enable(info, true),
            GroupUpdate::GroupUpdate(info) => self.sync.group = Some(info),
            GroupUpdate::NotInGroup | GroupUpdate::GroupLeft => {
                let was_in = self.sync_active();
                self.disable(was_in);
            }
            GroupUpdate::StateUpdate { state, .. } => self.sync.group_state = state,
            GroupUpdate::SyncPlayIsDisabled => self.notice("Permission required to use SyncPlay."),
            GroupUpdate::GroupDoesNotExist => self.notice("Failed to join group because it does not exist."),
            GroupUpdate::CreateGroupDenied => self.notice("Permission required to create a group."),
            GroupUpdate::JoinGroupDenied => self.notice("Cannot join group."),
            GroupUpdate::LibraryAccessDenied => self.notice("Access to this content is restricted."),
        }
        Task::none()
    }

    // ── View ─────────────────────────────────────────────────────────────────

    /// The SyncPlay menu (jellyfin-web's group selection menu), in the
    /// context drawer.
    pub(super) fn view_syncplay(&self) -> Element<'_, Message> {
        let msg = |m: SyncMsg| Message::Sync(m);
        let mut col = widget::column::with_capacity(8).spacing(12);
        let entry = |title: String, subtitle: String, icon: &'static str, on: Message| -> Element<'_, Message> {
            widget::button::custom(
                widget::row::with_capacity(2)
                    .push(widget::icon::from_name(icon).size(20))
                    .push(
                        widget::column::with_capacity(2)
                            .push(widget::text::body(title))
                            .push(widget::text::caption(subtitle))
                            .spacing(2),
                    )
                    .spacing(12)
                    .align_y(Alignment::Center),
            )
            .on_press(on)
            .padding([8, 12])
            .width(Length::Fill)
            .class(cosmic::theme::Button::MenuItem)
            .into()
        };

        if let Some(group) = &self.sync.group {
            col = col
                .push(widget::text::title4(group.group_name.clone()))
                .push(widget::text::body(group.participants.join(", ")));
            if !self.sync.group_state.is_empty() {
                col = col.push(widget::text::caption(format!("Group: {}", self.sync.group_state)));
            }
            let has_queue = self.sync.queue.as_ref().is_some_and(|q| !q.playlist.is_empty());
            if self.playback_active() {
                col = col.push(entry("Stop local playback".into(), "And ignore current playlist updates".into(), "media-playback-pause-symbolic", msg(SyncMsg::Halt)));
            } else if has_queue {
                col = col.push(entry("Resume local playback".into(), "Join back group playback".into(), "media-playback-start-symbolic", msg(SyncMsg::Resume)));
            }
            col = col.push(entry("Leave group".into(), "Disable SyncPlay".into(), "system-log-out-symbolic", msg(SyncMsg::Leave)));

            col = col.push(widget::divider::horizontal::default());
            col = col.push(widget::text::heading("Playback"));
            col = col.push(
                widget::settings::item::builder("Sync Correction")
                    .description("Enable active syncing of playback by either speeding up the media or by seeking to the estimated position. Disable this in case of heavy stuttering.")
                    .control(widget::toggler(self.settings.sync_correction).on_toggle(|on| Message::Sync(SyncMsg::SetCorrection(on)))),
            );
            col = col.push(
                widget::settings::item::builder("Enable SpeedToSync")
                    .description("Sync correction method that consist in speeding up the playback. Sync Correction must be enabled.")
                    .control(widget::toggler(self.settings.speed_to_sync).on_toggle(|on| Message::Sync(SyncMsg::SetSpeedToSync(on)))),
            );
            col = col.push(
                widget::settings::item::builder("Enable SkipToSync")
                    .description("Sync correction method that consist in seeking to the estimated position. Sync Correction must be enabled.")
                    .control(widget::toggler(self.settings.skip_to_sync).on_toggle(|on| Message::Sync(SyncMsg::SetSkipToSync(on)))),
            );
            col = col.push(widget::text::heading("SyncPlay Info"));
            let info = |k: &'static str, v: String| {
                widget::row::with_capacity(2)
                    .push(widget::text::body(k).width(Length::Fill))
                    .push(widget::text::body(v))
            };
            col = col
                .push(info("Time syncing with", "Server".into()))
                .push(info("Time offset", format!("{:.2} ms", self.sync.time.offset())))
                .push(info("Playback time difference", format!("{:.2} ms", self.sync.diff_ms)))
                .push(info("Sync method", self.sync.method.clone().unwrap_or_else(|| "None".into())));
        } else {
            col = col.push(widget::text::title4("Join a group"));
            match &self.sync.groups {
                None => col = col.push(widget::text::body("Loading…")),
                Some(Err(e)) => {
                    log::warn!("SyncPlay groups: {e}");
                    col = col.push(widget::text::body("An error occurred while accessing groups list."));
                }
                Some(Ok(groups)) => {
                    if groups.is_empty() {
                        col = col.push(widget::text::caption("No groups available"));
                    }
                    for g in groups {
                        col = col.push(entry(g.group_name.clone(), g.participants.join(", "), "avatar-default-symbolic", msg(SyncMsg::Join(g.group_id.clone()))));
                    }
                }
            }
            col = col.push(entry("New group".into(), "Create a new group".into(), "list-add-symbolic", msg(SyncMsg::NewGroup)));
        }
        col.into()
    }

    /// The latest notices ("Tim has joined the group."), bottom centre.
    pub(super) fn sync_notices(&self) -> Option<Element<'_, Message>> {
        if self.sync.notices.is_empty() {
            return None;
        }
        let mut col = widget::column::with_capacity(self.sync.notices.len()).spacing(6).align_x(Alignment::Center);
        for (text, _) in &self.sync.notices {
            col = col.push(
                widget::container(widget::text::body(text.clone()))
                    .padding([8, 16])
                    .class(cosmic::theme::Container::custom(crate::theme::notice)),
            );
        }
        Some(
            widget::container(col)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_bottom(Length::Fill)
                .padding([0, 0, 32, 0])
                .into(),
        )
    }
}
