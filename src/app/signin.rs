//! Connecting to a Jellyfin server: Quick Connect when the server offers
//! it, username and password otherwise (or as well).

use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::Element;

use super::{App, Message};

impl App {
    pub(super) fn view_signin(&self) -> Element<'_, Message> {
        let s = &self.signin;
        let mut col = widget::column::with_capacity(8).spacing(16).max_width(520);
        col = col.push(widget::text::title1("Nebula Player"));
        col = col.push(widget::text::body("Sign in to your Jellyfin server to see your movies and shows."));

        let mut connect = widget::button::suggested("Connect");
        if !s.busy && !s.server.trim().is_empty() {
            connect = connect.on_press(Message::Connect);
        }
        col = col.push(
            widget::row::with_capacity(2)
                .push(
                    widget::text_input("Server address, e.g. 192.168.1.5:8096", &s.server)
                        .on_input(Message::ServerInput)
                        .on_submit(|_| Message::Connect)
                        .width(Length::Fill),
                )
                .push(connect)
                .spacing(8)
                .align_y(Alignment::Center),
        );

        if let Some((url, name)) = &s.found {
            col = col.push(widget::text::heading(format!("Found {name} at {url}")));
            if let Some(qc) = &s.quick_connect {
                col = col.push(
                    widget::column::with_capacity(3)
                        .push(widget::text::body(
                            "Approve this code in Jellyfin (your profile → Quick Connect) on a device that's signed in:",
                        ))
                        .push(widget::text::title1(qc.code.clone()))
                        .push(widget::text::caption("Waiting for approval…"))
                        .spacing(8),
                );
                col = col.push(widget::text::caption("Or sign in with your password:"));
            }
            let mut sign_in = widget::button::suggested("Sign In");
            if !s.busy && !s.user.trim().is_empty() {
                sign_in = sign_in.on_press(Message::PasswordSignIn);
            }
            col = col
                .push(widget::text_input("Username", &s.user).on_input(Message::UserInput))
                .push(
                    widget::secure_input("Password", &s.password, None, true)
                        .on_input(Message::PasswordInput)
                        .on_submit(|_| Message::PasswordSignIn),
                )
                .push(sign_in);
        }
        if s.busy {
            col = col.push(widget::text::caption("Working…"));
        }
        if let Some(e) = &s.error {
            col = col.push(widget::text::body(e.clone()));
        }
        col = col.push(widget::Space::new().width(Length::Shrink).height(Length::Fixed(12.0)));
        col = col.push(widget::button::text("Just play local files").on_press(Message::OpenFile));

        widget::container(widget::scrollable(widget::container(col).padding(32)).height(Length::Shrink))
            .center(Length::Fill)
            .into()
    }
}
