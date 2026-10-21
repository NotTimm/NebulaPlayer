//! The settings drawer.

use cosmic::iced::Length;
use cosmic::widget;
use cosmic::Element;

use super::{App, Message};
use crate::settings::{AccentChoice, SkipMode, ThemeChoice};

const THEME_LABELS: [&str; 2] = ["Jellyfin", "System (COSMIC)"];

impl App {
    pub(super) fn view_settings(&self) -> Element<'_, Message> {
        let s = &self.settings;
        let mut account = widget::settings::section().title("Jellyfin");
        match &self.account {
            Some(a) => {
                account = account.add(widget::settings::item(
                    a.label(),
                    widget::button::destructive("Sign Out").on_press(Message::SignOut),
                ));
            }
            None => {
                account = account.add(widget::settings::item_row(vec![widget::text::body("Not signed in").into()]));
            }
        }
        for other in &self.accounts.accounts {
            if Some(other.key()) == self.account.as_ref().map(|a| a.key()) {
                continue;
            }
            account = account.add(widget::settings::item(
                other.label(),
                widget::button::standard("Switch").on_press(Message::SwitchAccount(other.key())),
            ));
        }
        account = account.add(widget::settings::item_row(vec![
            widget::button::standard("Add a Server or User").on_press(Message::AddAccount).into(),
        ]));

        let mut look = widget::settings::section().title("Appearance").add(widget::settings::item(
            "Colors",
            widget::dropdown(
                &THEME_LABELS[..],
                Some(if s.theme == ThemeChoice::System { 1 } else { 0 }),
                Message::SetTheme,
            ),
        ));
        // The desktop's theme brings its own accent.
        if s.theme == ThemeChoice::Jellyfin {
            look = look.add(widget::settings::item(
                "Accent color",
                widget::dropdown(
                    &AccentChoice::LABELS[..],
                    AccentChoice::ALL.iter().position(|a| *a == s.accent),
                    Message::SetAccent,
                ),
            ));
        }

        let playback = widget::settings::section()
            .title("Playback (mpv)")
            .add(widget::settings::item("Start full screen", widget::toggler(s.fullscreen).on_toggle(Message::SetFullscreen)));
        // macOS always plays in mpv's own window.
        let playback = if crate::mpv::separate_only() {
            playback
        } else {
            playback.add(widget::settings::item(
                "Play in mpv's own window",
                widget::toggler(s.separate_window).on_toggle(Message::SetSeparateWindow),
            ))
        };
        let playback = playback
            .add(widget::settings::item("Hardware decoding", widget::toggler(s.hwdec).on_toggle(Message::SetHwdec)))
            .add(widget::settings::item(
                "Play the next episode",
                widget::toggler(s.autoplay_next).on_toggle(Message::SetAutoplay),
            ))
            .add(widget::settings::item(
                "Intros and credits",
                widget::dropdown(
                    &SkipMode::LABELS[..],
                    SkipMode::ALL.iter().position(|m| *m == s.skip),
                    Message::SetSkip,
                ),
            ))
            .add(widget::settings::item(
                "mpv.conf, input.conf, scripts",
                widget::button::standard("Open Folder").on_press(Message::OpenMpvConfig),
            ));

        let about = widget::settings::section().title("About").add(widget::settings::item_row(vec![
            widget::text::caption(match crate::mpv::api_version() {
                Some(v) => format!("Nebula Player {} · {v}", env!("CARGO_PKG_VERSION")),
                None => format!("Nebula Player {} · {}", env!("CARGO_PKG_VERSION"), crate::mpv::INSTALL_HINT),
            })
            .into(),
        ]));

        widget::column::with_capacity(4)
            .push(account)
            .push(look)
            .push(playback)
            .push(about)
            .spacing(24)
            .width(Length::Fill)
            .into()
    }
}
