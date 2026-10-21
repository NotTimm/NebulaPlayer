//! A library or folder: a grid of posters, sortable, fetched a page at a
//! time as the user scrolls down.

use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::Element;

use super::{App, LibraryPage, Message};
use crate::jellyfin::SortBy;

const SORT_LABELS: [&str; 4] = ["Name", "Date Added", "Release Date", "Rating"];

impl App {
    pub(super) fn view_library<'a>(&'a self, lib: &'a LibraryPage) -> Element<'a, Message> {
        let count = if lib.total > 0 { format!("{} items", lib.total) } else { String::new() };
        let header = widget::row::with_capacity(4)
            .push(widget::text::title2(lib.parent.name.clone()))
            .push(widget::text::caption(count))
            .push(widget::Space::new().width(Length::Fill).height(Length::Shrink))
            .push(widget::text::body("Sort by"))
            .push(widget::dropdown(
                &SORT_LABELS[..],
                SortBy::ALL.iter().position(|s| *s == lib.sort),
                Message::LibrarySort,
            ))
            .spacing(12)
            .align_y(Alignment::Center);

        let grid = widget::flex_row(lib.items.iter().map(|i| self.poster_card(i)).collect())
            .column_spacing(18)
            .row_spacing(22);

        let mut col = widget::column::with_capacity(3).spacing(20).padding([20, 28, 28, 28]).push(header);
        if lib.items.is_empty() && !lib.loading {
            col = col.push(widget::text::body("Nothing here yet."));
        }
        col = col.push(grid);
        if lib.loading {
            col = col.push(widget::text::caption("Loading…"));
        }
        // Near the bottom, the next page loads.
        widget::scrollable(col)
            .height(Length::Fill)
            .on_scroll(|v| {
                if v.relative_offset().y > 0.85 {
                    Message::LibraryMore
                } else {
                    Message::Cancelled
                }
            })
            .into()
    }
}
