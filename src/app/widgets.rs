//! Cards, rows and the other pieces the pages are built from.

use cosmic::iced::widget::stack;
use cosmic::iced::{Alignment, ContentFit, Length};
use cosmic::widget::{self, icon, image::Handle};
use cosmic::Element;

use super::{App, Key, Message, LANDSCAPE_W, PERSON_W, POSTER_W};
use crate::jellyfin::{Item, Person};

pub const POSTER: (f32, f32) = (150.0, 225.0);
pub const LANDSCAPE: (f32, f32) = (272.0, 153.0);
pub const PERSON: (f32, f32) = (110.0, 150.0);

/// Shortens `s` to at most `max` characters, with an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

pub fn error_banner(msg: &str) -> Element<'_, Message> {
    widget::container(
        widget::row::with_capacity(3)
            .push(icon::from_name("dialog-warning-symbolic").size(16))
            .push(widget::text::body(msg).width(Length::Fill))
            .push(widget::button::icon(icon::from_name("window-close-symbolic")).on_press(Message::DismissError))
            .spacing(12)
            .align_y(Alignment::Center),
    )
    .padding([8, 16])
    .class(cosmic::theme::Container::Card)
    .width(Length::Fill)
    .into()
}

/// A picture, or a tinted box with an icon while it loads.
pub fn picture<'a>(handle: Option<&Handle>, w: f32, h: f32, fallback: &'static str) -> Element<'a, Message> {
    match handle {
        Some(h_) => widget::image(h_.clone())
            .width(Length::Fixed(w))
            .height(Length::Fixed(h))
            .content_fit(ContentFit::Cover)
            .border_radius([6.0; 4])
            .into(),
        None => widget::container(icon::from_name(fallback).size(32))
            .width(Length::Fixed(w))
            .height(Length::Fixed(h))
            .center_x(Length::Fixed(w))
            .center_y(Length::Fixed(h))
            .class(cosmic::theme::Container::custom(crate::theme::placeholder))
            .into(),
    }
}

/// A thin bar along a card's bottom edge.
pub fn progress<'a>(fraction: f32, width: f32) -> Element<'a, Message> {
    cosmic::iced::widget::ProgressBar::new(0.0..=1.0, fraction)
        .girth(4.0)
        .length(Length::Fixed(width))
        .into()
}

fn icon_for(item: &Item) -> &'static str {
    match item.kind.as_str() {
        "Series" | "Season" | "Episode" => "video-display-symbolic",
        "Movie" => "video-x-generic-symbolic",
        "BoxSet" | "Folder" | "CollectionFolder" => "folder-videos-symbolic",
        _ => "video-x-generic-symbolic",
    }
}

/// The corner badge: a tick once watched, the unwatched episode count for
/// a show.
fn badge<'a>(item: &Item) -> Option<Element<'a, Message>> {
    let content: Element<'a, Message> = if item.played() {
        icon::from_name("object-select-symbolic").size(14).into()
    } else {
        match item.user_data.as_ref().and_then(|u| u.unplayed_item_count) {
            Some(n) if n > 0 && (item.is_series() || item.kind == "Season") => widget::text::caption(n.to_string()).into(),
            _ => return None,
        }
    };
    Some(
        widget::container(content)
            .padding([2, 7])
            .class(cosmic::theme::Container::custom(crate::theme::badge))
            .into(),
    )
}

/// The picture with its badge and progress bar laid over it.
fn overlaid<'a>(pic: Element<'a, Message>, item: &Item, w: f32, h: f32) -> Element<'a, Message> {
    let mut layers = stack![pic];
    if let Some(b) = badge(item) {
        layers = layers.push(widget::container(b).width(Length::Fixed(w)).align_right(Length::Fixed(w)).padding(6));
    }
    if let Some(p) = item.progress().filter(|_| !item.played()) {
        layers = layers.push(
            widget::container(progress(p, w - 16.0))
                .width(Length::Fixed(w))
                .height(Length::Fixed(h))
                .align_bottom(Length::Fixed(h))
                .padding([0, 8, 8, 8]),
        );
    }
    layers.into()
}

fn clickable<'a>(content: impl Into<Element<'a, Message>>, msg: Message) -> Element<'a, Message> {
    widget::button::custom(content)
        .class(crate::theme::card_button())
        .padding(6)
        .on_press(msg)
        .into()
}

/// Second line under a card's title.
pub fn subtitle(item: &Item) -> String {
    if item.is_episode() {
        return match (item.episode_code(), &item.series_name) {
            (Some(code), Some(series)) => format!("{series} · {code}"),
            (Some(code), None) => code,
            (None, Some(series)) => series.clone(),
            _ => String::new(),
        };
    }
    if item.is_series() {
        let start = item.production_year.map(|y| y.to_string()).unwrap_or_default();
        return match (item.status.as_deref(), &item.end_date) {
            (Some("Continuing"), _) if !start.is_empty() => format!("{start} – Present"),
            (_, Some(end)) if !start.is_empty() && end.len() >= 4 && &end[..4] != start => format!("{start} – {}", &end[..4]),
            _ => start,
        };
    }
    item.production_year.map(|y| y.to_string()).unwrap_or_default()
}

impl App {
    /// A portrait card (movies, shows, seasons); opens its page.
    pub fn poster_card<'a>(&'a self, item: &'a Item) -> Element<'a, Message> {
        let (w, h) = POSTER;
        let key = item.poster().map(|img| Key { img, width: POSTER_W });
        let pic = picture(key.as_ref().and_then(|k| self.image(k)), w, h, icon_for(item));
        let title = match item.kind.as_str() {
            "Episode" => item.series_name.clone().unwrap_or_else(|| item.name.clone()),
            _ => item.name.clone(),
        };
        let card = widget::column::with_capacity(3)
            .push(overlaid(pic, item, w, h))
            .push(widget::text::body(truncate(&title, 22)))
            .push(widget::text::caption(subtitle(item)))
            .spacing(4)
            .width(Length::Fixed(w));
        clickable(card, Message::Open(item.clone()))
    }

    /// A 16:9 card. Continue Watching and Next Up cards play straight
    /// away (`plays`); others open the item's page.
    pub fn landscape_card<'a>(&'a self, item: &'a Item, plays: bool) -> Element<'a, Message> {
        let (w, h) = LANDSCAPE;
        let key = item.landscape().map(|img| Key { img, width: LANDSCAPE_W });
        let pic = picture(key.as_ref().and_then(|k| self.image(k)), w, h, icon_for(item));
        let (title, sub) = if item.is_episode() {
            let code = item.episode_code().map(|c| format!("{c} · ")).unwrap_or_default();
            (item.series_name.clone().unwrap_or_default(), format!("{code}{}", item.name))
        } else {
            (item.name.clone(), subtitle(item))
        };
        let card = widget::column::with_capacity(3)
            .push(overlaid(pic, item, w, h))
            .push(widget::text::body(truncate(&title, 36)))
            .push(widget::text::caption(truncate(&sub, 44)))
            .spacing(4)
            .width(Length::Fixed(w));
        let msg = if plays {
            Message::Play { item: item.clone(), resume: true }
        } else {
            Message::Open(item.clone())
        };
        clickable(card, msg)
    }

    /// A library on the home screen's My Media row.
    pub fn library_tile<'a>(&'a self, lib: &'a Item) -> Element<'a, Message> {
        let (w, h) = LANDSCAPE;
        let key = lib.landscape().map(|img| Key { img, width: LANDSCAPE_W });
        let card = widget::column::with_capacity(2)
            .push(picture(key.as_ref().and_then(|k| self.image(k)), w, h, "folder-videos-symbolic"))
            .push(widget::text::body(truncate(&lib.name, 36)))
            .spacing(4)
            .width(Length::Fixed(w));
        clickable(card, Message::Open(lib.clone()))
    }

    pub fn person_card<'a>(&'a self, p: &'a Person) -> Element<'a, Message> {
        let (w, h) = PERSON;
        let key = p.image().map(|img| Key { img, width: PERSON_W });
        let role = p.role.clone().filter(|r| !r.is_empty()).or_else(|| p.kind.clone()).unwrap_or_default();
        widget::column::with_capacity(3)
            .push(picture(key.as_ref().and_then(|k| self.image(k)), w, h, "avatar-default-symbolic"))
            .push(widget::text::body(truncate(&p.name, 16)))
            .push(widget::text::caption(truncate(&role, 18)))
            .spacing(4)
            .width(Length::Fixed(w))
            .into()
    }
}

/// A titled row of cards that scrolls sideways.
pub fn section<'a>(title: impl Into<String>, cards: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let row = widget::row::with_children(cards).spacing(16).padding([0, 0, 14, 0]);
    widget::column::with_capacity(2)
        .push(widget::text::title4(title.into()))
        .push(widget::scrollable::horizontal(row).width(Length::Fill))
        .spacing(10)
        .into()
}

/// The meta line under a title: "2021 · PG-13 · 1h 52m · ★ 7.4".
pub fn meta_line(item: &Item) -> String {
    let mut parts: Vec<String> = Vec::new();
    let year = subtitle(item);
    if !year.is_empty() && !item.is_episode() {
        parts.push(year);
    }
    if let Some(r) = &item.official_rating {
        parts.push(r.clone());
    }
    if let Some(m) = item.runtime_minutes() {
        parts.push(if m >= 60 { format!("{}h {}m", m / 60, m % 60) } else { format!("{m}m") });
    }
    if let Some(r) = item.community_rating {
        parts.push(format!("★ {r:.1}"));
    }
    if let Some(c) = item.critic_rating {
        parts.push(format!("🍅 {c:.0}%"));
    }
    parts.join("  ·  ")
}

/// The big gradient Play button.
pub fn play_button<'a>(label: impl Into<String>, msg: Message) -> Element<'a, Message> {
    widget::button::custom(
        widget::row::with_capacity(2)
            .push(icon::from_name("media-playback-start-symbolic").size(16))
            .push(widget::text::heading(label.into()))
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([10, 22])
    .class(crate::theme::play_button())
    .on_press(msg)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_characters() {
        assert_eq!(truncate("Short", 10), "Short");
        assert_eq!(truncate("Ünïcödé title here", 8), "Ünïcödé…");
    }
}
