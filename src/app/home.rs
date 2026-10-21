//! The home screen: a featured banner, then the rows Jellyfin's web client
//! shows (My Media, Continue Watching, Next Up, Recently Added per library).

use cosmic::iced::widget::stack;
use cosmic::iced::{Alignment, ContentFit, Length};
use cosmic::widget::{self, icon};
use cosmic::Element;

use super::widgets::{self, section, truncate};
use super::{App, Key, Message, BACKDROP_W, LOGO_W};
use crate::jellyfin::Item;

const HERO_H: f32 = 460.0;
/// The logo's box on the banner.
const LOGO_BOX: (f32, f32) = (420.0, 130.0);

impl App {
    pub(super) fn view_home(&self) -> Element<'_, Message> {
        let home = &self.home;
        if !home.loaded {
            let label = if home.loading { "Loading your library…" } else { "Nothing loaded yet." };
            return widget::container(widget::text::title4(label))
                .center(Length::Fill)
                .into();
        }

        let mut col = widget::column::with_capacity(6).spacing(28);
        if let Some(item) = home.featured.get(home.featured_at) {
            col = col.push(self.hero(item));
        }

        let mut rows = widget::column::with_capacity(5).spacing(28).padding([0, 24, 24, 24]);
        if !home.libraries.is_empty() {
            rows = rows.push(section("My Media", home.libraries.iter().map(|l| self.library_tile(l)).collect()));
        }
        if !home.resume.is_empty() {
            rows = rows.push(section(
                "Continue Watching",
                home.resume.iter().map(|i| self.landscape_card(i, true)).collect(),
            ));
        }
        if !home.next_up.is_empty() {
            rows = rows.push(section("Next Up", home.next_up.iter().map(|i| self.landscape_card(i, true)).collect()));
        }
        for (lib, items) in &home.latest {
            rows = rows.push(section(
                format!("Recently Added in {}", lib.name),
                items.iter().map(|i| self.poster_card(i)).collect(),
            ));
        }
        if home.libraries.is_empty() {
            rows = rows.push(widget::text::body(
                "This server has no movie or TV libraries for your account yet.",
            ));
        }
        col = col.push(rows);
        widget::scrollable(col).height(Length::Fill).into()
    }

    /// The featured banner (what the Media Bar plugin does in Jellyfin's
    /// web client): a backdrop, the title art, a line about it and Play.
    fn hero<'a>(&'a self, item: &'a Item) -> Element<'a, Message> {
        let backdrop: Element<'_, Message> = match item.backdrop().and_then(|b| self.image(&Key { img: b, width: BACKDROP_W })) {
            Some(h) => widget::image(h.clone())
                .width(Length::Fill)
                .height(Length::Fixed(HERO_H))
                .content_fit(ContentFit::Cover)
                .into(),
            None => widget::Space::new().width(Length::Fill).height(Length::Fixed(HERO_H)).into(),
        };
        let shade = |style: fn(&cosmic::Theme) -> widget::container::Style| {
            widget::container(widget::Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fixed(HERO_H))
                .class(cosmic::theme::Container::custom(style))
        };

        let title: Element<'_, Message> = match item.logo().and_then(|l| self.picture(&Key { img: l, width: LOGO_W })) {
            // Sized to fit its box keeping its shape, so it sits flush left.
            Some(p) if p.width > 0 && p.height > 0 => {
                let scale = (LOGO_BOX.0 / p.width as f32).min(LOGO_BOX.1 / p.height as f32);
                widget::image(p.handle.clone())
                    .width(Length::Fixed(p.width as f32 * scale))
                    .height(Length::Fixed(p.height as f32 * scale))
                    .content_fit(ContentFit::Contain)
                    .into()
            }
            _ => widget::text::title1(item.name.clone()).into(),
        };
        let mut info = widget::column::with_capacity(5).spacing(12).max_width(620);
        info = info.push(title);
        let meta = widgets::meta_line(item);
        let genres = item.genres.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        let line = [meta, genres].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  ·  ");
        if !line.is_empty() {
            info = info.push(widget::text::body(line));
        }
        if let Some(o) = &item.overview {
            info = info.push(widget::text::body(truncate(o, 240)));
        }
        let resume = item.progress().is_some();
        info = info.push(
            widget::row::with_capacity(2)
                .push(widgets::play_button(if resume { "Resume" } else { "Play" }, Message::Play { item: item.clone(), resume }))
                .push(widget::button::standard("More Info").on_press(Message::Open(item.clone())))
                .spacing(12)
                .align_y(Alignment::Center),
        );

        // One dot per featured item; the current one is the accent.
        let dots = widget::row::with_children(
            (0..self.home.featured.len())
                .map(|i| {
                    let name = if i == self.home.featured_at { "media-record-symbolic" } else { "radio-symbolic" };
                    widget::button::icon(icon::from_name(name).size(10))
                        .padding(2)
                        .on_press(Message::FeaturedPick(i))
                        .into()
                })
                .collect::<Vec<_>>(),
        )
        .spacing(2);
        info = info.push(dots);

        let content = widget::container(info)
            .width(Length::Fill)
            .height(Length::Fixed(HERO_H))
            .align_bottom(Length::Fixed(HERO_H))
            .padding([24, 56, 16, 56]);
        let arrows = widget::row::with_capacity(3)
            .push(widget::button::icon(icon::from_name("go-previous-symbolic")).on_press(Message::FeaturedStep(-1)))
            .push(widget::Space::new().width(Length::Fill).height(Length::Shrink))
            .push(widget::button::icon(icon::from_name("go-next-symbolic")).on_press(Message::FeaturedStep(1)))
            .align_y(Alignment::Center)
            .padding([0, 8])
            .height(Length::Fixed(HERO_H));

        stack![
            backdrop,
            shade(crate::theme::side_fade),
            shade(crate::theme::backdrop_fade),
            content,
            arrows,
        ]
        .into()
    }
}
