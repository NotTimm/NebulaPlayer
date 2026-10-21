//! The player screen: mpv's video in the app's window, under controls like
//! Jellyfin Media Player's: a title bar with Back, a seek bar with times,
//! previous/next episode, back 10 s and forward 30 s, play/pause, volume,
//! audio, subtitle and speed menus, full screen, and a Skip Intro button
//! when the server knows one. They fade out while the video plays and come
//! back when the mouse moves. The keys are jellyfin-web's.

use std::time::{Duration, Instant};

use cosmic::iced::widget::{shader, stack};
use cosmic::iced::{keyboard, mouse, window, Alignment, Length};
use cosmic::widget::{self, icon};
use cosmic::{Application, Element};

use super::{App, Message};
use crate::mpv;

/// Jellyfin's default skip lengths.
const BACK: f64 = 10.0;
const FORWARD: f64 = 30.0;
/// Controls hide after the mouse has been still this long.
const HIDE_AFTER: Duration = Duration::from_secs(3);
const SPEEDS: [f64; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

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
            && ui.last_activity.elapsed() >= HIDE_AFTER
        {
            ui.controls = false;
        }
    }

    pub(super) fn update_player(&mut self, msg: PlayerMsg) -> cosmic::app::Task<Message> {
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
            PlayerMsg::SeekDrag(s) => self.player_ui.seek_drag = Some(s),
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

        let area = widget::mouse_area(layers)
            .on_move(|_| Message::Player(PlayerMsg::Activity))
            .on_press(Message::Player(PlayerMsg::TogglePause))
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
        let top = widget::container(
            widget::row::with_capacity(2)
                .push(overlay_icon("go-previous-symbolic", 20, msg(PlayerMsg::Stop)))
                .push(widget::text::title4(title))
                .spacing(12)
                .align_y(Alignment::Center),
        )
        .padding([16, 20, 40, 20])
        .width(Length::Fill)
        .class(cosmic::theme::Container::custom(|t| crate::theme::player_fade(t, true)));

        let position = ui.seek_drag.unwrap_or(s.position);
        let duration = s.duration.max(position).max(0.1);
        let seek = widget::row::with_capacity(3)
            .push(widget::text::body(clock(position)))
            .push(
                widget::slider(0.0..=duration, position, |v| Message::Player(PlayerMsg::SeekDrag(v)))
                    .step(0.5)
                    .on_release(msg(PlayerMsg::SeekRelease))
                    .width(Length::Fill),
            )
            .push(widget::text::body(format!("-{}", clock(duration - position))))
            .spacing(14)
            .align_y(Alignment::Center);

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

        let bottom = widget::container(widget::column::with_capacity(2).push(seek).push(buttons).spacing(6))
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
