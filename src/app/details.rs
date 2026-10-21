//! A movie's, show's or episode's page: backdrop, poster, what it is, Play,
//! and below that the seasons and episodes, the cast and similar titles.

use cosmic::iced::widget::stack;
use cosmic::iced::{Alignment, ContentFit, Length};
use cosmic::widget::{self, icon};
use cosmic::Element;

use super::widgets::{self, picture, section, truncate, LANDSCAPE};
use super::{App, DetailsPage, Key, Message, BACKDROP_W, LANDSCAPE_W, POSTER_W};
use crate::jellyfin::Item;

const BANNER_H: f32 = 380.0;
const POSTER: (f32, f32) = (200.0, 300.0);
const EPISODE_THUMB: (f32, f32) = (224.0, 126.0);

impl App {
    pub(super) fn view_details<'a>(&'a self, d: &'a DetailsPage) -> Element<'a, Message> {
        let item = &d.item;
        let mut col = widget::column::with_capacity(6).spacing(24);
        col = col.push(self.banner(item));

        let mut body = widget::column::with_capacity(6).spacing(28).padding([0, 32, 32, 32]);
        body = body.push(self.summary(d));

        if item.is_series() {
            body = body.push(self.seasons_and_episodes(d));
        }
        let cast: Vec<Element<'_, Message>> = item
            .people
            .iter()
            .filter(|p| matches!(p.kind.as_deref(), Some("Actor" | "GuestStar" | "Director")))
            .take(30)
            .map(|p| self.person_card(p))
            .collect();
        if !cast.is_empty() {
            body = body.push(section("Cast & Crew", cast));
        }
        if !d.similar.is_empty() {
            body = body.push(section("More Like This", d.similar.iter().map(|i| self.poster_card(i)).collect()));
        }
        if d.loading {
            body = body.push(widget::text::caption("Loading…"));
        }
        col = col.push(body);
        widget::scrollable(col).height(Length::Fill).into()
    }

    /// The backdrop, fading into the page.
    fn banner<'a>(&'a self, item: &'a Item) -> Element<'a, Message> {
        let backdrop: Element<'_, Message> = match item.backdrop().and_then(|b| self.image(&Key { img: b, width: BACKDROP_W })) {
            Some(h) => widget::image(h.clone())
                .width(Length::Fill)
                .height(Length::Fixed(BANNER_H))
                .content_fit(ContentFit::Cover)
                .into(),
            None => widget::Space::new().width(Length::Fill).height(Length::Fixed(BANNER_H)).into(),
        };
        stack![
            backdrop,
            widget::container(widget::Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fixed(BANNER_H))
                .class(cosmic::theme::Container::custom(crate::theme::backdrop_fade)),
        ]
        .into()
    }

    /// Poster, title, facts, overview and the buttons.
    fn summary<'a>(&'a self, d: &'a DetailsPage) -> Element<'a, Message> {
        let item = &d.item;
        let poster_key = item.poster().map(|img| Key { img, width: POSTER_W });
        let poster = if item.is_episode() {
            let key = item.landscape().map(|img| Key { img, width: LANDSCAPE_W });
            picture(key.as_ref().and_then(|k| self.image(k)), LANDSCAPE.0 * 1.2, LANDSCAPE.1 * 1.2, "video-display-symbolic")
        } else {
            picture(poster_key.as_ref().and_then(|k| self.image(k)), POSTER.0, POSTER.1, "video-x-generic-symbolic")
        };

        let mut info = widget::column::with_capacity(8).spacing(10).width(Length::Fill);
        if item.is_episode() {
            if let Some(series) = &item.series_name {
                let code = item.episode_code().map(|c| format!("  ·  {c}")).unwrap_or_default();
                info = info.push(widget::text::title4(format!("{series}{code}")));
            }
        }
        info = info.push(widget::text::title1(item.name.clone()));
        let meta = widgets::meta_line(item);
        if !meta.is_empty() {
            info = info.push(widget::text::body(meta));
        }
        if !item.genres.is_empty() {
            info = info.push(widget::text::caption(item.genres.join(", ")));
        }
        if let Some(t) = item.taglines.first() {
            info = info.push(widget::text::heading(t.clone()));
        }

        // Play / Resume, start over, watched and favourite.
        let mut buttons = widget::row::with_capacity(5).spacing(10).align_y(Alignment::Center);
        if item.is_series() {
            let label = match &d.next_up {
                Some(n) => match n.episode_code() {
                    Some(code) => format!("Play {code}"),
                    None => "Play".into(),
                },
                None => "Play".into(),
            };
            let target = d.next_up.clone().unwrap_or_else(|| item.clone());
            let resume = target.progress().is_some();
            buttons = buttons.push(widgets::play_button(label, Message::Play { item: target, resume }));
        } else if item.is_playable() {
            let resume = item.progress().is_some();
            buttons = buttons.push(widgets::play_button(
                if resume { "Resume" } else { "Play" },
                Message::Play { item: item.clone(), resume },
            ));
            if resume {
                buttons = buttons.push(
                    widget::button::standard("From the Start").on_press(Message::Play { item: item.clone(), resume: false }),
                );
            }
        }
        let watched = item.played();
        buttons = buttons.push(super::tip(
            widget::button::icon(icon::from_name(if watched { "object-select-symbolic" } else { "emblem-default-symbolic" }))
                .selected(watched)
                .on_press(Message::TogglePlayed(item.clone())),
            if watched { "Watched (click to unmark)" } else { "Mark watched" },
        ));
        let fav = item.favorite();
        buttons = buttons.push(super::tip(
            widget::button::icon(icon::from_name(if fav { "starred-symbolic" } else { "non-starred-symbolic" }))
                .selected(fav)
                .on_press(Message::ToggleFavorite(item.clone())),
            if fav { "Favorite (click to remove)" } else { "Add to favorites" },
        ));
        info = info.push(buttons);

        if let Some(o) = &item.overview {
            info = info.push(widget::text::body(o.clone()));
        }
        let directors: Vec<&str> = item
            .people
            .iter()
            .filter(|p| p.kind.as_deref() == Some("Director"))
            .map(|p| p.name.as_str())
            .collect();
        if !directors.is_empty() {
            info = info.push(widget::text::caption(format!("Directed by {}", directors.join(", "))));
        }
        if !item.studios.is_empty() {
            let studios: Vec<&str> = item.studios.iter().map(|s| s.name.as_str()).collect();
            info = info.push(widget::text::caption(studios.join(", ")));
        }

        widget::row::with_capacity(2).push(poster).push(info).spacing(32).into()
    }

    /// Season tabs and the chosen season's episodes.
    fn seasons_and_episodes<'a>(&'a self, d: &'a DetailsPage) -> Element<'a, Message> {
        let mut col = widget::column::with_capacity(3).spacing(16);
        if d.seasons.is_empty() {
            return col.into();
        }
        let tabs = widget::row::with_children(
            d.seasons
                .iter()
                .map(|s| {
                    let selected = d.season.as_deref() == Some(s.id.as_str());
                    widget::button::text(s.name.clone())
                        .class(if selected { cosmic::theme::Button::Suggested } else { cosmic::theme::Button::Text })
                        .on_press(Message::SeasonPick(s.id.clone()))
                        .into()
                })
                .collect::<Vec<_>>(),
        )
        .spacing(6);
        col = col.push(widget::scrollable::horizontal(tabs.padding([0, 0, 12, 0])));

        if d.episodes_loading {
            col = col.push(widget::text::caption("Loading episodes…"));
        }
        for ep in &d.episodes {
            col = col.push(self.episode_row(ep));
        }
        col.into()
    }

    fn episode_row<'a>(&'a self, ep: &'a Item) -> Element<'a, Message> {
        let (w, h) = EPISODE_THUMB;
        let key = ep.landscape().map(|img| Key { img, width: LANDSCAPE_W });
        let mut thumb = stack![picture(key.as_ref().and_then(|k| self.image(k)), w, h, "video-display-symbolic")];
        if let Some(p) = ep.progress().filter(|_| !ep.played()) {
            thumb = thumb.push(
                widget::container(widgets::progress(p, w - 16.0))
                    .width(Length::Fixed(w))
                    .height(Length::Fixed(h))
                    .align_bottom(Length::Fixed(h))
                    .padding([0, 8, 8, 8]),
            );
        }

        let number = ep.index_number.map(|n| format!("{n}. ")).unwrap_or_default();
        let mut text = widget::column::with_capacity(3)
            .push(widget::text::heading(format!("{number}{}", ep.name)))
            .spacing(4)
            .width(Length::Fill);
        let mut facts = Vec::new();
        if let Some(m) = ep.runtime_minutes() {
            facts.push(format!("{m} min"));
        }
        if let Some(date) = ep.premiere_date.as_deref().and_then(|d| d.get(..10)) {
            facts.push(date.to_string());
        }
        if let Some(r) = ep.community_rating {
            facts.push(format!("★ {r:.1}"));
        }
        if !facts.is_empty() {
            text = text.push(widget::text::caption(facts.join("  ·  ")));
        }
        if let Some(o) = &ep.overview {
            text = text.push(widget::text::body(truncate(o, 260)));
        }

        let watched = ep.played();
        let tick = super::tip(
            widget::button::icon(icon::from_name(if watched { "object-select-symbolic" } else { "emblem-default-symbolic" }))
                .selected(watched)
                .on_press(Message::TogglePlayed(ep.clone())),
            if watched { "Watched" } else { "Mark watched" },
        );
        let row = widget::row::with_capacity(3)
            .push(thumb)
            .push(text)
            .push(tick)
            .spacing(20)
            .align_y(Alignment::Center);
        widget::button::custom(row)
            .class(cosmic::theme::Button::ListItem([8.0; 4]))
            .padding(8)
            .width(Length::Fill)
            .on_press(Message::Play { item: ep.clone(), resume: true })
            .into()
    }
}
