//! Search across the server's movies, shows and episodes.

use std::sync::LazyLock;

use cosmic::iced::Length;
use cosmic::widget;
use cosmic::Element;

use super::widgets::section;
use super::{App, Message};

pub static INPUT_ID: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("search"));

impl App {
    pub(super) fn view_search(&self) -> Element<'_, Message> {
        let input = widget::search_input("Search movies, shows and episodes", &self.search.term)
            .id(INPUT_ID.clone())
            .on_input(Message::SearchInput)
            .on_submit(|_| Message::SearchSubmit)
            .width(Length::Fill);
        let mut col = widget::column::with_capacity(5).spacing(28).padding([20, 28, 28, 28]).push(input);
        if self.search.loading {
            col = col.push(widget::text::caption("Searching…"));
        }
        let s = &self.search;
        let group = |kinds: &[&str]| s.results.iter().filter(|i| kinds.contains(&i.kind.as_str())).collect::<Vec<_>>();
        let movies = group(&["Movie", "BoxSet"]);
        let shows = group(&["Series"]);
        let episodes = group(&["Episode"]);
        if !movies.is_empty() {
            col = col.push(section("Movies", movies.into_iter().map(|i| self.poster_card(i)).collect()));
        }
        if !shows.is_empty() {
            col = col.push(section("Shows", shows.into_iter().map(|i| self.poster_card(i)).collect()));
        }
        if !episodes.is_empty() {
            col = col.push(section("Episodes", episodes.into_iter().map(|i| self.landscape_card(i, false)).collect()));
        }
        if !s.searched.is_empty() && s.results.is_empty() && !s.loading {
            col = col.push(widget::text::body(format!("Nothing found for “{}”.", s.searched)));
        }
        super::widgets::page_scroll(col)
    }
}
