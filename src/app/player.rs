//! The player screen: mpv's video in the app's window, under controls like
//! Jellyfin Media Player's: a title bar with Back, a seek bar with times,
//! previous/next episode, back 10 s and forward 30 s, play/pause, volume,
//! audio, subtitle and speed menus, full screen, and a Skip Intro button
//! when the server knows one. They fade out while the video plays and come
//! back when the mouse moves. The keys are jellyfin-web's.
//!
//! Hovering on (or dragging) the seek bar shows the time there, its
//! chapter, and the server's trickplay thumbnail for it. The thumbnails
//! come as the server made them, many to a sheet; a sheet is fetched the
//! first time the seek bar needs it (once: they're cached on disk too),
//! never ahead, so scrubbing costs a movie's few sheets at most.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use cosmic::iced::widget::{shader, stack};
use cosmic::iced::{keyboard, mouse, window, Alignment, Length};
use cosmic::widget::{self, icon, image::Handle};
use cosmic::{Application, Element};

use super::{App, Message};
use crate::images::SheetKey;
use crate::jellyfin::TrickplayInfo;
use crate::mpv;

/// Jellyfin's default skip lengths.
const BACK: f64 = 10.0;
const FORWARD: f64 = 30.0;
/// Controls hide after the mouse has been still this long.
const HIDE_AFTER: Duration = Duration::from_secs(3);
const SPEEDS: [f64; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];
/// The times either side of the seek bar, a fixed width so the preview
/// can be placed over the bar.
const TIME_W: f32 = 64.0;
const SEEK_SPACING: f32 = 14.0;
/// The preview thumbnail's width on screen.
const PREVIEW_W: f32 = 240.0;
/// Sheets kept in memory (about 23 MB each at the default size); the ones
/// furthest from where the user is looking go first. They stay on disk.
const KEEP_SHEETS: usize = 6;

/// The playing item's trickplay thumbnails.
pub struct Trickplay {
    info: TrickplayInfo,
    item_id: String,
    source_id: String,
    sheets: HashMap<u32, Vec<Handle>>,
    /// Sheets on their way: never asked for twice at once.
    pending: HashSet<u32>,
    /// Sheets the server didn't have: not asked for again.
    failed: HashSet<u32>,
}

impl Trickplay {
    pub fn new(item_id: String, source_id: String, info: TrickplayInfo) -> Self {
        Trickplay { info, item_id, source_id, sheets: HashMap::new(), pending: HashSet::new(), failed: HashSet::new() }
    }

    fn thumbnail(&self, seconds: f64) -> Option<&Handle> {
        let (sheet, tile) = self.info.locate(seconds)?;
        self.sheets.get(&sheet)?.get(tile as usize)
    }

    /// The preview's size on screen, keeping the thumbnails' shape.
    fn preview_size(&self) -> (f32, f32) {
        (PREVIEW_W, (PREVIEW_W * self.info.height as f32 / self.info.width.max(1) as f32).round())
    }

    fn keep(&mut self, index: u32, tiles: Vec<Handle>) {
        self.sheets.insert(index, tiles);
        while self.sheets.len() > KEEP_SHEETS {
            let far = *self.sheets.keys().max_by_key(|k| k.abs_diff(index)).unwrap();
            self.sheets.remove(&far);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Audio,
    Subtitles,
    Speed,
}

/// The player screen's own state.
pub struct PlayerUi {
    /// mpv's state when last looked at.
    pub state: mpv::State,
    last_activity: Instant,
    pub controls: bool,
    /// Where the seek bar is being dragged to.
    seek_drag: Option<f64>,
    menu: Option<Menu>,
    pub fullscreen: bool,
    /// Where the pointer is along the seek bar, and the bar's width.
    hover: Option<f32>,
    seek_width: f32,
}

impl Default for PlayerUi {
    fn default() -> Self {
        PlayerUi {
            state: mpv::State::default(),
            last_activity: Instant::now(),
            controls: true,
            seek_drag: None,
            menu: None,
            fullscreen: false,
            hover: None,
            seek_width: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum PlayerMsg {
    /// mpv drew a new frame.
    Frame,
    /// The mouse moved: show the controls.
    Activity,
    TogglePause,
    SeekBy(f64),
    SeekDrag(f64),
    SeekRelease,
    /// The pointer moved along the seek bar (or left it).
    SeekHover(Option<f32>),
    SeekWidth(f32),
    /// A trickplay sheet for item, by number.
    Sheet(String, u32, Result<Vec<Handle>, String>),
    Volume(f64),
    ToggleMute,
    Audio(i64),
    Subtitle(Option<i64>),
    Speed(f64),
    Menu(Option<Menu>),
    ToggleFullscreen,
    Stop,
    Skip,
    Next,
    Previous,
    Key(keyboard::Event),
}

/// "1:02:03" or "4:05".
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A ringing frame bell turned into messages, for the subscription.
pub fn frames() -> impl cosmic::iced::futures::Stream<Item = Message> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::stream::channel(1, |mut tx: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
        loop {
            crate::video::FRAME_READY.notified().await;
            if tx.send(Message::Player(PlayerMsg::Frame)).await.is_err() {
                break;
            }
        }
    })
}

pub fn key_message(event: keyboard::Event) -> Message {
    Message::Player(PlayerMsg::Key(event))
}

impl App {
    /// Whether mpv is drawing into this window right now.
    pub(super) fn playing_inside(&self) -> bool {
        self.player.as_ref().is_some_and(|p| p.video().is_some() && !self.player_ui.state.closed)
    }

    /// Switches the window over to the player.
    pub(super) fn enter_player(&mut self) -> cosmic::app::Task<Message> {
        self.player_ui = PlayerUi::default();
        if let Some(p) = &self.player {
            self.player_ui.state = p.state();
        }
        self.core.window.show_headerbar = false;
        self.core.window.show_context = false;
        if self.settings.fullscreen {
            return self.set_fullscreen(true);
        }
        cosmic::app::Task::none()
    }

    /// Back to the pages, as they were.
    pub(super) fn leave_player(&mut self) -> cosmic::app::Task<Message> {
        self.core.window.show_headerbar = true;
        let task = if self.player_ui.fullscreen { self.set_fullscreen(false) } else { cosmic::app::Task::none() };
        self.player_ui = PlayerUi::default();
        // The page comes back from scratch, scrolled to the top: once it's
        // there, back to where it was.
        let y = self.scroll_y;
        let rescroll = cosmic::task::future(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Message::RestoreScroll(y)
        });
        cosmic::app::Task::batch([task, rescroll])
    }

    fn set_fullscreen(&mut self, on: bool) -> cosmic::app::Task<Message> {
        self.player_ui.fullscreen = on;
        let Some(id) = self.core.main_window_id() else { return cosmic::app::Task::none() };
        let mode = if on { window::Mode::Fullscreen } else { window::Mode::Windowed };
        window::set_mode(id, mode)
    }

    /// Shows the controls, for a while if the video is playing.
    fn wake_controls(&mut self) {
        self.player_ui.last_activity = Instant::now();
        self.player_ui.controls = true;
    }

    /// Hides the controls once the mouse has been still a while, unless
    /// paused or a menu is open.
    pub(super) fn player_idle_check(&mut self) {
        let ui = &mut self.player_ui;
        if ui.controls
            && !ui.state.paused
            && ui.menu.is_none()
            && ui.seek_drag.is_none()
            && ui.hover.is_none()
            && ui.last_activity.elapsed() >= HIDE_AFTER
        {
            ui.controls = false;
        }
    }

    pub(super) fn update_player(&mut self, msg: PlayerMsg) -> cosmic::app::Task<Message> {
        // Playing a SyncPlay group's item: playback is the group's.
        if self.sync_playing() {
            if let Some(task) = self.group_player_msg(&msg) {
                self.wake_controls();
                return task;
            }
        }
        let Some(player) = &self.player else { return cosmic::app::Task::none() };
        let state = &self.player_ui.state;
        match msg {
            PlayerMsg::Frame => return cosmic::app::Task::none(),
            PlayerMsg::Activity => {
                self.wake_controls();
                return cosmic::app::Task::none();
            }
            PlayerMsg::TogglePause => player.set_paused(!state.paused),
            PlayerMsg::SeekBy(s) => player.seek_by(s),
            PlayerMsg::SeekDrag(s) => {
                self.player_ui.seek_drag = Some(s);
                return self.want_sheet();
            }
            PlayerMsg::SeekHover(x) => {
                self.player_ui.hover = x;
                return self.want_sheet();
            }
            PlayerMsg::SeekWidth(w) => {
                self.player_ui.seek_width = w;
                return cosmic::app::Task::none();
            }
            PlayerMsg::Sheet(item_id, index, result) => {
                let Some(tp) = self.playback.as_mut().and_then(|pb| pb.trickplay.as_mut()) else {
                    return cosmic::app::Task::none();
                };
                if tp.item_id != item_id {
                    return cosmic::app::Task::none();
                }
                tp.pending.remove(&index);
                match result {
                    Ok(tiles) => tp.keep(index, tiles),
                    Err(e) => {
                        log::info!("trickplay sheet {index}: {e}");
                        tp.failed.insert(index);
                    }
                }
                // The pointer may have moved on to another sheet meanwhile.
                return self.want_sheet();
            }
            PlayerMsg::SeekRelease => {
                if let Some(s) = self.player_ui.seek_drag.take() {
                    player.seek(s);
                }
            }
            PlayerMsg::Volume(v) => {
                player.set_volume(v);
                if state.muted {
                    player.set_muted(false);
                }
            }
            PlayerMsg::ToggleMute => player.set_muted(!state.muted),
            PlayerMsg::Audio(id) => {
                player.set_audio(id);
                self.player_ui.menu = None;
            }
            PlayerMsg::Subtitle(id) => {
                player.set_subtitle(id);
                self.player_ui.menu = None;
            }
            PlayerMsg::Speed(s) => {
                player.set_speed(s);
                self.player_ui.menu = None;
            }
            PlayerMsg::Menu(m) => self.player_ui.menu = if self.player_ui.menu == m { None } else { m },
            PlayerMsg::ToggleFullscreen => {
                let on = !self.player_ui.fullscreen;
                return self.set_fullscreen(on);
            }
            PlayerMsg::Stop => return self.stop_playback(),
            PlayerMsg::Skip => self.skip_now(),
            PlayerMsg::Next => {
                if let Some(next) = self.playback.as_ref().and_then(|pb| pb.next.clone()) {
                    return self.update(Message::Play { item: next, resume: false });
                }
            }
            PlayerMsg::Previous => {
                // Like a music player: back to the start, unless already
                // near it.
                let prev = self.playback.as_ref().and_then(|pb| pb.previous.clone());
                match prev {
                    Some(prev) if state.position < 5.0 => return self.update(Message::Play { item: prev, resume: false }),
                    _ => player.seek(0.0),
                }
            }
            PlayerMsg::Key(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                if let Some(msg) = self.key_action(&key, modifiers) {
                    self.wake_controls();
                    return self.update_player(msg);
                }
                return cosmic::app::Task::none();
            }
            PlayerMsg::Key(_) => return cosmic::app::Task::none(),
        }
        self.wake_controls();
        if let Some(p) = &self.player {
            self.player_ui.state = p.state();
        }
        cosmic::app::Task::none()
    }

    /// The player's buttons and keys in a SyncPlay group, as jellyfin-web
    /// routes them: requests to the group instead of the player. `None`
    /// for what stays local (volume, tracks, full screen, stop).
    fn group_player_msg(&mut self, msg: &PlayerMsg) -> Option<cosmic::app::Task<Message>> {
        let position = self.player_ui.state.position;
        Some(match msg {
            PlayerMsg::TogglePause => self.group_toggle_pause(),
            PlayerMsg::SeekBy(s) => self.group_seek(position + s),
            PlayerMsg::SeekRelease => match self.player_ui.seek_drag.take() {
                Some(s) => self.group_seek(s),
                None => cosmic::app::Task::none(),
            },
            PlayerMsg::Next => self.group_next(true),
            PlayerMsg::Previous => {
                let has_prev = self.playback.as_ref().is_some_and(|pb| pb.previous.is_some());
                if has_prev && position < 5.0 {
                    self.group_next(false)
                } else {
                    self.group_seek(0.0)
                }
            }
            // jellyfin-web ignores speed changes in a group.
            PlayerMsg::Speed(_) => {
                self.player_ui.menu = None;
                cosmic::app::Task::none()
            }
            PlayerMsg::Key(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                let action = self.key_action(key, *modifiers)?;
                return self.group_player_msg(&action);
            }
            _ => return None,
        })
    }

    /// jellyfin-web's keys.
    fn key_action(&self, key: &keyboard::Key, modifiers: keyboard::Modifiers) -> Option<PlayerMsg> {
        use keyboard::key::Named;
        let state = &self.player_ui.state;
        Some(match key.as_ref() {
            keyboard::Key::Named(Named::ArrowLeft) => PlayerMsg::SeekBy(-BACK),
            keyboard::Key::Named(Named::ArrowRight) => PlayerMsg::SeekBy(FORWARD),
            keyboard::Key::Named(Named::ArrowUp) => PlayerMsg::Volume((state.volume + 5.0).min(100.0)),
            keyboard::Key::Named(Named::ArrowDown) => PlayerMsg::Volume((state.volume - 5.0).max(0.0)),
            keyboard::Key::Named(Named::Enter) if self.skip_prompt().is_some() => PlayerMsg::Skip,
            keyboard::Key::Character(c) => match c.to_lowercase().as_str() {
                " " | "k" => PlayerMsg::TogglePause,
                "j" => PlayerMsg::SeekBy(-BACK),
                "l" => PlayerMsg::SeekBy(FORWARD),
                "f" => PlayerMsg::ToggleFullscreen,
                "m" => PlayerMsg::ToggleMute,
                "s" if self.skip_prompt().is_some() => PlayerMsg::Skip,
                "n" if modifiers.shift() => PlayerMsg::Next,
                "p" if modifiers.shift() => PlayerMsg::Previous,
                // 0–9 jump to that tenth of the way through.
                d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && state.duration > 0.0 => {
                    let tenth = (d.as_bytes()[0] - b'0') as f64;
                    PlayerMsg::SeekBy(state.duration * tenth / 10.0 - state.position)
                }
                _ => return None,
            },
            _ => return None,
        })
    }

    /// The time the seek bar's preview is for: where it's dragged to, or
    /// where the pointer is over it.
    fn preview_time(&self) -> Option<f64> {
        let ui = &self.player_ui;
        let duration = ui.state.duration;
        if duration <= 0.0 {
            return None;
        }
        ui.seek_drag.or_else(|| {
            let x = ui.hover?;
            (ui.seek_width > 0.0).then(|| (x / ui.seek_width).clamp(0.0, 1.0) as f64 * duration)
        })
    }

    /// Fetches the trickplay sheet the preview needs, if it isn't here,
    /// on its way, or known to be missing.
    fn want_sheet(&mut self) -> cosmic::app::Task<Message> {
        let time = self.preview_time();
        let Some(pb) = self.playback.as_mut() else { return cosmic::app::Task::none() };
        let (Some(time), Some(tp), Some(remote)) = (time, pb.trickplay.as_mut(), pb.remote.as_ref()) else {
            return cosmic::app::Task::none();
        };
        let Some((index, _)) = tp.info.locate(time) else { return cosmic::app::Task::none() };
        if tp.sheets.contains_key(&index) || tp.failed.contains(&index) || !tp.pending.insert(index) {
            return cosmic::app::Task::none();
        }
        let key = SheetKey { item_id: tp.item_id.clone(), source_id: tp.source_id.clone(), info: tp.info, index };
        let account = remote.account.clone();
        cosmic::task::future(async move {
            let item_id = key.item_id.clone();
            let result = crate::images::load_sheet(account, key).await;
            Message::Player(PlayerMsg::Sheet(item_id, index, result))
        })
    }

    /// The bubble over the seek bar: the thumbnail, the time, the chapter.
    fn seek_preview(&self, time: f64) -> Element<'_, Message> {
        let pb = self.playback.as_ref();
        let mut col = widget::column::with_capacity(3).spacing(4).align_x(Alignment::Center);
        if let Some(tp) = pb.and_then(|pb| pb.trickplay.as_ref()) {
            let (w, h) = tp.preview_size();
            col = col.push(match tp.thumbnail(time) {
                Some(handle) => Element::from(
                    widget::image(handle.clone())
                        .width(Length::Fixed(w))
                        .height(Length::Fixed(h))
                        .border_radius([6.0; 4]),
                ),
                // Its sheet is on the way.
                None => widget::container(widget::Space::new().width(Length::Fixed(w)).height(Length::Fixed(h)))
                    .class(cosmic::theme::Container::custom(crate::theme::player_menu))
                    .into(),
            });
        }
        let chapter = pb
            .and_then(|pb| pb.chapters.iter().take_while(|c| c.start() <= time).last())
            .and_then(|c| c.name.clone())
            .filter(|n| !n.is_empty());
        if let Some(name) = chapter {
            col = col.push(widget::text::caption(name));
        }
        col = col.push(widget::text::heading(clock(time)));
        widget::container(col).padding(6).class(cosmic::theme::Container::custom(crate::theme::player_menu)).into()
    }

    /// The label of the segment whose Skip button is up.
    fn skip_prompt(&self) -> Option<&'static str> {
        let pb = self.playback.as_ref()?;
        pb.prompting.map(|i| pb.segments[i].kind.label())
    }

    pub(super) fn view_player(&self) -> Element<'_, Message> {
        let ui = &self.player_ui;
        let Some(output) = self.player.as_ref().and_then(|p| p.video()) else {
            return widget::Space::new().width(Length::Fill).height(Length::Fill).into();
        };
        let video = widget::container(shader(crate::video::Video { output }).width(Length::Fill).height(Length::Fill))
            .class(cosmic::theme::Container::custom(crate::theme::black))
            .width(Length::Fill)
            .height(Length::Fill);

        let mut layers = stack![video].width(Length::Fill).height(Length::Fill);
        if ui.state.buffering || (!ui.state.closed && ui.state.duration == 0.0) {
            layers = layers.push(
                widget::container(widget::text::title3("Loading…"))
                    .center(Length::Fill)
                    .class(cosmic::theme::Container::custom(crate::theme::on_video)),
            );
        }
        if ui.controls {
            layers = layers.push(self.player_controls());
        } else if let Some(label) = self.skip_prompt() {
            // The Skip button stays up without the rest of the controls.
            layers = layers.push(
                widget::container(skip_button(label))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_right(Length::Fill)
                    .align_bottom(Length::Fill)
                    .padding([0, 40, 48, 0]),
            );
        }

        if let Some(notices) = self.sync_notices() {
            layers = layers.push(notices);
        }

        // A click only shows the controls (the owner's choice: pausing is for
        // the pause button and keys); a double click is full screen.
        let area = widget::mouse_area(layers)
            .on_move(|_| Message::Player(PlayerMsg::Activity))
            .on_press(Message::Player(PlayerMsg::Activity))
            .on_double_click(Message::Player(PlayerMsg::ToggleFullscreen));
        if ui.controls {
            area.into()
        } else {
            area.interaction(mouse::Interaction::Hidden).into()
        }
    }

    fn player_controls(&self) -> Element<'_, Message> {
        let ui = &self.player_ui;
        let s = &ui.state;
        let pb = self.playback.as_ref();
        let msg = |m: PlayerMsg| Message::Player(m);

        let title = pb.map(|pb| pb.title.clone()).unwrap_or_default();
        let mut top_row = widget::row::with_capacity(4)
            .push(overlay_icon("go-previous-symbolic", 20, msg(PlayerMsg::Stop)))
            .push(widget::text::title4(title).width(Length::Fill))
            .spacing(12)
            .align_y(Alignment::Center);
        // SyncPlay: what correction is going on, and the group menu.
        if let Some(method) = self.sync.method.clone() {
            top_row = top_row.push(widget::text::caption(method));
        }
        if self.account.is_some() {
            top_row = top_row.push(overlay_icon("system-users-symbolic", 20, Message::Sync(super::SyncMsg::ToggleMenu)));
        }
        let top = widget::container(top_row)
        .padding([16, 20, 40, 20])
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(|t| crate::theme::player_fade(t, true)));

        let position = ui.seek_drag.unwrap_or(s.position);
        let duration = s.duration.max(position).max(0.1);
        let slider = widget::slider(0.0..=duration, position, |v| Message::Player(PlayerMsg::SeekDrag(v)))
            .step(0.5)
            .on_release(msg(PlayerMsg::SeekRelease))
            .width(Length::Fill);
        // Its width (to turn the pointer's place into a time) and the
        // pointer's place along it.
        let slider = widget::mouse_area(
            cosmic::iced::widget::sensor(slider)
                .on_show(|size| Message::Player(PlayerMsg::SeekWidth(size.width)))
                .on_resize(|size| Message::Player(PlayerMsg::SeekWidth(size.width))),
        )
        .on_move(|p| Message::Player(PlayerMsg::SeekHover(Some(p.x))))
        .on_exit(msg(PlayerMsg::SeekHover(None)));
        let seek = widget::row::with_capacity(3)
            .push(widget::container(widget::text::body(clock(position))).width(Length::Fixed(TIME_W)))
            .push(slider)
            .push(
                widget::container(widget::text::body(format!("-{}", clock(duration - position))))
                    .width(Length::Fixed(TIME_W))
                    .align_right(Length::Fixed(TIME_W)),
            )
            .spacing(SEEK_SPACING)
            .align_y(Alignment::Center);

        // The preview sits over the point it's for, kept inside the bar's
        // row; the controls under it don't move when it comes and goes.
        let preview = self.preview_time().map(|time| {
            let bubble_w = self.playback.as_ref().and_then(|pb| pb.trickplay.as_ref()).map_or(90.0, |tp| tp.preview_size().0 + 12.0);
            let at = TIME_W + SEEK_SPACING + (time / duration) as f32 * ui.seek_width;
            let row_w = 2.0 * (TIME_W + SEEK_SPACING) + ui.seek_width;
            let left = (at - bubble_w / 2.0).clamp(0.0, (row_w - bubble_w).max(0.0));
            widget::row::with_capacity(2)
                .push(widget::Space::new().width(Length::Fixed(left)).height(Length::Shrink))
                .push(self.seek_preview(time))
        });

        // Previous and next are for episodes; a movie has neither.
        let episodic = pb.is_some_and(|pb| pb.previous.is_some() || pb.next.is_some());
        let has_prev = pb.is_some_and(|pb| pb.previous.is_some());
        let has_next = pb.is_some_and(|pb| pb.next.is_some());
        let mut transport = widget::row::with_capacity(5).spacing(10).align_y(Alignment::Center);
        if episodic {
            transport = transport.push(overlay_icon_maybe("media-skip-backward-symbolic", 20, has_prev.then(|| msg(PlayerMsg::Previous))));
        }
        transport = transport.push(overlay_icon("media-seek-backward-symbolic", 22, msg(PlayerMsg::SeekBy(-BACK))));
        let play_icon = if s.paused { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" };
        transport = transport.push(overlay_icon(play_icon, 30, msg(PlayerMsg::TogglePause)));
        transport = transport.push(overlay_icon("media-seek-forward-symbolic", 22, msg(PlayerMsg::SeekBy(FORWARD))));
        if episodic {
            transport = transport.push(overlay_icon_maybe("media-skip-forward-symbolic", 20, has_next.then(|| msg(PlayerMsg::Next))));
        }

        let volume_icon = if s.muted || s.volume <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if s.volume < 50.0 {
            "audio-volume-low-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        let has = |kind: &str| s.tracks.iter().any(|t| t.kind == kind);
        let mut extras = widget::row::with_capacity(6).spacing(6).align_y(Alignment::Center);
        extras = extras
            .push(overlay_icon(volume_icon, 20, msg(PlayerMsg::ToggleMute)))
            .push(
                widget::slider(0.0..=100.0, if s.muted { 0.0 } else { s.volume.min(100.0) }, |v| Message::Player(PlayerMsg::Volume(v)))
                    .width(Length::Fixed(110.0)),
            );
        if s.tracks.iter().filter(|t| t.kind == "audio").count() > 1 {
            extras = extras.push(overlay_icon("audio-x-generic-symbolic", 20, msg(PlayerMsg::Menu(Some(Menu::Audio)))));
        }
        if has("sub") {
            extras = extras.push(overlay_icon("media-view-subtitles-symbolic", 20, msg(PlayerMsg::Menu(Some(Menu::Subtitles)))));
        }
        extras = extras.push(
            widget::button::custom(widget::text::body(format!("{}×", trim_speed(s.speed))))
                .class(crate::theme::overlay_button())
                .padding([6, 10])
                .on_press(msg(PlayerMsg::Menu(Some(Menu::Speed)))),
        );
        let full = if ui.fullscreen { "view-restore-symbolic" } else { "view-fullscreen-symbolic" };
        extras = extras.push(overlay_icon(full, 20, msg(PlayerMsg::ToggleFullscreen)));

        let buttons = widget::row::with_capacity(3)
            .push(widget::container(transport).width(Length::Fill))
            .push(extras)
            .align_y(Alignment::Center);

        let bottom = widget::container(widget::column::with_capacity(3).push_maybe(preview).push(seek).push(buttons).spacing(6))
            .padding([48, 24, 16, 24])
            .width(Length::Fill)
            .class(cosmic::theme::Container::custom(|t| crate::theme::player_fade(t, false)));

        let mut over = widget::column::with_capacity(4)
            .push(top)
            .push(widget::Space::new().width(Length::Fill).height(Length::Fill));
        if let Some(menu) = ui.menu {
            over = over.push(widget::container(self.player_menu(menu)).width(Length::Fill).align_right(Length::Fill).padding([0, 24, 8, 0]));
        }
        if let Some(label) = self.skip_prompt() {
            over = over.push(widget::container(skip_button(label)).width(Length::Fill).align_right(Length::Fill).padding([0, 40, 8, 0]));
        }
        widget::container(over.push(bottom).width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .class(cosmic::theme::Container::custom(crate::theme::on_video))
            .into()
    }

    /// The list a menu button opens.
    fn player_menu(&self, menu: Menu) -> Element<'_, Message> {
        let s = &self.player_ui.state;
        let item = |label: String, selected: bool, m: Message| -> Element<'_, Message> {
            let mark: Element<'_, Message> = if selected {
                icon::from_name("object-select-symbolic").size(16).into()
            } else {
                widget::Space::new().width(Length::Fixed(16.0)).height(Length::Fixed(16.0)).into()
            };
            widget::button::custom(
                widget::row::with_capacity(2).push(mark).push(widget::text::body(label)).spacing(10).align_y(Alignment::Center),
            )
            .class(crate::theme::overlay_button())
            .padding([6, 12])
            .width(Length::Fill)
            .on_press(m)
            .into()
        };
        let mut col = widget::column::with_capacity(8).spacing(2);
        let title = match menu {
            Menu::Audio => "Audio",
            Menu::Subtitles => "Subtitles",
            Menu::Speed => "Playback speed",
        };
        col = col.push(widget::container(widget::text::heading(title)).padding([4, 12, 6, 12]));
        match menu {
            Menu::Audio => {
                for t in s.tracks.iter().filter(|t| t.kind == "audio") {
                    col = col.push(item(t.label(), t.selected, Message::Player(PlayerMsg::Audio(t.id))));
                }
            }
            Menu::Subtitles => {
                let any = s.tracks.iter().any(|t| t.kind == "sub" && t.selected);
                col = col.push(item("Off".into(), !any, Message::Player(PlayerMsg::Subtitle(None))));
                for t in s.tracks.iter().filter(|t| t.kind == "sub") {
                    col = col.push(item(t.label(), t.selected, Message::Player(PlayerMsg::Subtitle(Some(t.id)))));
                }
            }
            Menu::Speed => {
                for sp in SPEEDS {
                    let label = if sp == 1.0 { "Normal".to_string() } else { format!("{}×", trim_speed(sp)) };
                    col = col.push(item(label, (s.speed - sp).abs() < 0.01, Message::Player(PlayerMsg::Speed(sp))));
                }
            }
        }
        widget::container(widget::scrollable(col).height(Length::Shrink))
            .padding(6)
            .max_width(380)
            .max_height(420)
            .class(cosmic::theme::Container::custom(crate::theme::player_menu))
            .into()
    }
}

/// "1.25", "2", "0.5".
fn trim_speed(s: f64) -> String {
    let t = format!("{s:.2}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn overlay_icon<'a>(name: &'static str, size: u16, msg: Message) -> Element<'a, Message> {
    overlay_icon_maybe(name, size, Some(msg))
}

fn overlay_icon_maybe<'a>(name: &'static str, size: u16, msg: Option<Message>) -> Element<'a, Message> {
    widget::button::custom(icon::from_name(name).size(size))
        .class(crate::theme::overlay_button())
        .padding(8)
        .on_press_maybe(msg)
        .into()
}

fn skip_button<'a>(label: &str) -> Element<'a, Message> {
    widget::button::custom(
        widget::row::with_capacity(2)
            .push(widget::text::heading(format!("Skip {label}")))
            .push(icon::from_name("media-skip-forward-symbolic").size(16))
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .class(crate::theme::play_button())
    .padding([10, 20])
    .on_press(Message::Player(PlayerMsg::Skip))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_read_like_jellyfins() {
        assert_eq!(clock(5.0), "0:05");
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(clock(-3.0), "0:00");
        assert_eq!(trim_speed(1.25), "1.25");
        assert_eq!(trim_speed(2.0), "2");
        assert_eq!(trim_speed(0.5), "0.5");
    }
}
