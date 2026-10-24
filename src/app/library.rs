//! A library or folder: a grid of posters filling the width, sortable and
//! searchable, with an A–Z bar to jump through it when sorted by name.
//!
//! Only the rows on screen (and a couple either side) are built: a big
//! library has thousands of cards, and building and laying out all of them
//! on every scroll step made scrolling stutter. Spacers above and below
//! keep the scrollbar true to the whole library. Items are fetched a page
//! at a time, wherever the user is: a row of placeholders coming into view
//! asks for its page.

use std::cell::Cell;
use std::collections::HashSet;
use std::time::Duration;

use cosmic::iced::widget::{responsive, scrollable, sensor};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::widget;
use cosmic::app::Task;
use cosmic::Element;

use super::widgets::{CARD_PADDING, CARD_TEXT_H};
use super::{page_scroll_id, App, Message, PAGE_SIZE};
use crate::jellyfin::{self, Item, LibraryQuery, SortBy};

const SORT_LABELS: [&str; 4] = ["Name", "Date Added", "Release Date", "Rating"];
const LETTERS: &str = "#ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The grid's margins: the left matches the header, the right leaves room
/// for the scrollbar.
const PAD_LEFT: f32 = 22.0;
const PAD_RIGHT: f32 = 18.0;
const COL_GAP: f32 = 12.0;
const ROW_GAP: f32 = 14.0;
/// Posters are at least this wide; columns stretch them to fill the width.
const MIN_POSTER_W: f32 = 146.0;
/// Rows built beyond the ones on screen, either side.
const SPARE_ROWS: usize = 2;

pub struct LibraryPage {
    pub query: LibraryQuery,
    /// What's typed in the search box; it becomes the query's search a
    /// moment after typing stops.
    pub search_input: String,
    search_typed: u64,
    /// Every item in order, `None` until its page arrives.
    pub items: Vec<Option<Item>>,
    pub total: usize,
    /// Whether the first page has answered (so "nothing here" is true).
    pub loaded: bool,
    /// Pages on their way, by number.
    pages_loading: HashSet<usize>,
    /// Counts query changes; answers to an older query are dropped.
    generation: u64,
    /// How far down the grid is scrolled, in pixels.
    pub offset: f32,
    /// The grid's size when last laid out.
    size: Cell<Size>,
}

impl LibraryPage {
    pub fn new(parent: Item) -> Self {
        LibraryPage {
            query: LibraryQuery { parent, sort: SortBy::Name, search: String::new() },
            search_input: String::new(),
            search_typed: 0,
            items: Vec::new(),
            total: 0,
            loaded: false,
            pages_loading: HashSet::from([0]),
            generation: 0,
            offset: 0.0,
            size: Cell::new(Size::new(1200.0, 800.0)),
        }
    }

    /// A row's height and the gap under it, at the grid's last width.
    pub fn row_height(&self) -> f32 {
        Grid::at(self.size.get().width).row_h
    }

    /// Starts over for a changed query: everything is fetched again.
    fn reset(&mut self) {
        self.generation += 1;
        self.items.clear();
        self.total = 0;
        self.loaded = false;
        self.pages_loading = HashSet::from([0]);
        self.offset = 0.0;
    }
}

#[derive(Clone, Debug)]
pub enum LibraryMsg {
    /// A page of items from `start`, for query `generation`.
    Loaded(u64, usize, Result<(Vec<Item>, usize), String>),
    /// The item at this index came into view without having loaded.
    Need(usize),
    Sort(usize),
    SearchInput(String),
    /// Typing paused (the count says which keystroke it was after).
    SearchPause(u64),
    SearchSubmit,
    Jump(char),
    /// Where a letter starts, for query `generation`.
    Jumped(u64, Result<usize, String>),
}

/// How the grid is laid out at one width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cols: usize,
    /// A card's picture.
    pub poster: (f32, f32),
    /// One row and the gap under it.
    pub row_h: f32,
}

impl Grid {
    pub fn at(width: f32) -> Grid {
        let avail = (width - PAD_LEFT - PAD_RIGHT).max(MIN_POSTER_W);
        let min_cell = MIN_POSTER_W + 2.0 * CARD_PADDING;
        let cols = (((avail + COL_GAP) / (min_cell + COL_GAP)).floor() as usize).max(1);
        // Floored, so rounding never pushes the last card past the edge.
        let cell = ((avail - COL_GAP * (cols - 1) as f32) / cols as f32).floor();
        let w = cell - 2.0 * CARD_PADDING;
        let h = (w * 1.5).round();
        Grid { cols, poster: (w, h), row_h: h + CARD_TEXT_H + 2.0 * CARD_PADDING + ROW_GAP }
    }

    pub fn rows(&self, total: usize) -> usize {
        total.div_ceil(self.cols)
    }

    /// The rows to build when scrolled `offset` down a view `height` tall.
    pub fn visible(&self, total: usize, offset: f32, height: f32) -> std::ops::Range<usize> {
        let rows = self.rows(total);
        let first = ((offset / self.row_h).floor().max(0.0) as usize).saturating_sub(SPARE_ROWS);
        let last = (((offset + height) / self.row_h).ceil().max(0.0) as usize + SPARE_ROWS).min(rows);
        first.min(last)..last
    }
}

/// The letter a name files under in the A–Z bar ("#" for digits and
/// symbols), going by Jellyfin's sort name rules: no leading article.
pub fn letter_of(name: &str) -> char {
    let lower = name.trim().to_lowercase();
    let rest = ["the ", "a ", "an "].iter().find_map(|a| lower.strip_prefix(a)).unwrap_or(&lower);
    match rest.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        _ => '#',
    }
}

impl App {
    pub(super) fn open_library(&mut self, parent: Item, from_nav: bool) -> Task<Message> {
        let page = super::Page::Library(Box::new(LibraryPage::new(parent)));
        if from_nav {
            self.go(page);
        } else {
            self.push(page);
        }
        self.load_library_page(0)
    }

    /// Fetches page `n` of the open library.
    fn load_library_page(&self, n: usize) -> Task<Message> {
        let (Some(account), super::Page::Library(lib)) = (self.account.clone(), &self.page) else { return Task::none() };
        let (query, generation, start) = (lib.query.clone(), lib.generation, n * PAGE_SIZE);
        cosmic::task::future(async move {
            let result = jellyfin::library_page(&account, &query, start, PAGE_SIZE).await;
            Message::Library(LibraryMsg::Loaded(generation, start, result))
        })
    }

    /// Fetches the pages holding items `range` that haven't arrived.
    fn need_items(&mut self, range: std::ops::Range<usize>) -> Task<Message> {
        let super::Page::Library(lib) = &mut self.page else { return Task::none() };
        let end = range.end.min(lib.total);
        let pages: Vec<usize> = (range.start / PAGE_SIZE..end.div_ceil(PAGE_SIZE))
            .filter(|&p| {
                let missing = lib.items[p * PAGE_SIZE..((p + 1) * PAGE_SIZE).min(lib.total)].iter().any(Option::is_none);
                missing && lib.pages_loading.insert(p)
            })
            .collect();
        Task::batch(pages.into_iter().map(|p| self.load_library_page(p)))
    }

    /// Scrolls the grid to the row holding item `index`.
    fn scroll_to_item(&mut self, index: usize) -> Task<Message> {
        let super::Page::Library(lib) = &mut self.page else { return Task::none() };
        let grid = Grid::at(lib.size.get().width);
        let y = (index / grid.cols) as f32 * grid.row_h;
        lib.offset = y;
        let need = self.need_items(index..index + grid.cols * 8);
        Task::batch([
            need,
            scrollable::scroll_to(page_scroll_id(), scrollable::AbsoluteOffset { x: None, y: Some(y) }),
        ])
    }

    pub(super) fn update_library(&mut self, msg: LibraryMsg) -> Task<Message> {
        let sizes = self.sizes();
        let super::Page::Library(lib) = &mut self.page else { return Task::none() };
        match msg {
            LibraryMsg::Loaded(generation, start, result) => {
                if generation != lib.generation {
                    return Task::none();
                }
                lib.pages_loading.remove(&(start / PAGE_SIZE));
                match result {
                    Ok((items, total)) => {
                        lib.loaded = true;
                        if total != lib.total {
                            // The first answer, or the library changed.
                            lib.total = total;
                            lib.items.resize(total, None);
                        }
                        let keys = sizes.posters(&items);
                        for (i, item) in items.into_iter().enumerate() {
                            if let Some(slot) = lib.items.get_mut(start + i) {
                                *slot = Some(item);
                            }
                        }
                        return self.fetch_images(keys);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            LibraryMsg::Need(index) => {
                let cols = Grid::at(lib.size.get().width).cols;
                return self.need_items(index..index + cols);
            }
            LibraryMsg::Sort(i) => {
                let sort = SortBy::ALL.get(i).copied().unwrap_or_default();
                if sort != lib.query.sort {
                    lib.query.sort = sort;
                    lib.reset();
                    self.scroll_to(0.0);
                    return self.load_library_page(0);
                }
            }
            LibraryMsg::SearchInput(text) => {
                lib.search_input = text;
                lib.search_typed += 1;
                let typed = lib.search_typed;
                return cosmic::task::future(async move {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    Message::Library(LibraryMsg::SearchPause(typed))
                });
            }
            LibraryMsg::SearchPause(typed) if typed != lib.search_typed => {}
            LibraryMsg::SearchPause(_) | LibraryMsg::SearchSubmit => {
                let search = lib.search_input.trim().to_string();
                if search != lib.query.search {
                    lib.query.search = search;
                    lib.reset();
                    self.scroll_to(0.0);
                    return self.load_library_page(0);
                }
            }
            LibraryMsg::Jump(letter) => {
                if letter == '#' {
                    return self.scroll_to_item(0);
                }
                let Some(account) = self.account.clone() else { return Task::none() };
                let (query, generation) = (lib.query.clone(), lib.generation);
                return cosmic::task::future(async move {
                    let result = jellyfin::library_position(&account, &query, letter).await;
                    Message::Library(LibraryMsg::Jumped(generation, result))
                });
            }
            LibraryMsg::Jumped(generation, result) => {
                if generation != lib.generation {
                    return Task::none();
                }
                match result {
                    // Past the last item (nothing from that letter on):
                    // the end of the library.
                    Ok(index) => {
                        let last = lib.total.saturating_sub(1);
                        return self.scroll_to_item(index.min(last));
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        Task::none()
    }

    pub(super) fn view_library<'a>(&'a self, lib: &'a LibraryPage) -> Element<'a, Message> {
        let count = if lib.loaded { format!("{} items", lib.total) } else { String::new() };
        let search = widget::search_input(format!("Search {}", lib.query.parent.name), &lib.search_input)
            .on_input(|s| Message::Library(LibraryMsg::SearchInput(s)))
            .on_submit(|_| Message::Library(LibraryMsg::SearchSubmit))
            .on_clear(Message::Library(LibraryMsg::SearchInput(String::new())))
            .width(Length::Fixed(260.0));
        let header = widget::row::with_capacity(6)
            .push(widget::text::title2(lib.query.parent.name.clone()))
            .push(widget::text::caption(count))
            .push(widget::Space::new().width(Length::Fill).height(Length::Shrink))
            .push(search)
            .push(widget::text::body("Sort by"))
            .push(widget::dropdown(
                &SORT_LABELS[..],
                SortBy::ALL.iter().position(|s| *s == lib.query.sort),
                |i| Message::Library(LibraryMsg::Sort(i)),
            ))
            .spacing(12)
            .align_y(Alignment::Center)
            .padding([20, 28, 12, 28]);

        let body: Element<'a, Message> = if lib.loaded && lib.total == 0 {
            let empty = if lib.query.search.is_empty() {
                "Nothing here yet.".to_string()
            } else {
                format!("Nothing in {} matches “{}”.", lib.query.parent.name, lib.query.search)
            };
            widget::container(widget::text::body(empty)).padding([12, 28]).into()
        } else if !lib.loaded {
            widget::container(widget::text::caption("Loading…")).padding([12, 28]).into()
        } else {
            let grid = responsive(move |size| self.library_grid(lib, size));
            let mut row = widget::row::with_capacity(2).push(grid).height(Length::Fill);
            // Jumping by letter only makes sense in name order.
            if lib.query.sort == SortBy::Name {
                row = row.push(self.letter_bar(lib));
            }
            row.into()
        };

        widget::column::with_capacity(2).push(header).push(body).height(Length::Fill).into()
    }

    /// The rows on screen, between spacers standing in for the rest.
    fn library_grid<'a>(&'a self, lib: &'a LibraryPage, size: Size) -> Element<'a, Message> {
        lib.size.set(size);
        let grid = Grid::at(size.width);
        let (w, h) = grid.poster;
        let rows = grid.rows(lib.total);
        let visible = grid.visible(lib.total, lib.offset, size.height);

        let mut col = widget::column::with_capacity(visible.len() + 2).padding([0, PAD_RIGHT as u16, 28, PAD_LEFT as u16]);
        col = col.push(widget::Space::new().width(Length::Fill).height(Length::Fixed(visible.start as f32 * grid.row_h)));
        for r in visible.clone() {
            let start = r * grid.cols;
            let end = (start + grid.cols).min(lib.total);
            let mut cards = widget::row::with_capacity(grid.cols).spacing(COL_GAP);
            let mut missing = false;
            for slot in &lib.items[start..end] {
                cards = cards.push(match slot {
                    Some(item) => self.poster_card_sized(item, w, h),
                    None => {
                        missing = true;
                        Self::poster_placeholder(w, h)
                    }
                });
            }
            let cards = widget::container(cards).height(Length::Fixed(grid.row_h));
            col = col.push(if missing {
                // Asks for its page once it's (nearly) on screen.
                sensor(cards)
                    .key((lib.generation, r))
                    .anticipate(grid.row_h)
                    .on_show(move |_| Message::Library(LibraryMsg::Need(start)))
                    .into()
            } else {
                Element::from(cards)
            });
        }
        col = col.push(
            widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(rows.saturating_sub(visible.end) as f32 * grid.row_h)),
        );
        super::widgets::page_scroll(col)
    }

    /// "#" and A to Z down the right side; the letter at the top of the
    /// screen is lit.
    fn letter_bar<'a>(&'a self, lib: &'a LibraryPage) -> Element<'a, Message> {
        let grid = Grid::at(lib.size.get().width);
        let top = ((lib.offset / grid.row_h).floor().max(0.0) as usize) * grid.cols;
        let current = lib.items.get(top).and_then(|i| i.as_ref()).map(|i| letter_of(&i.name));
        let letters = LETTERS.chars().map(|c| {
            widget::button::custom(widget::text::caption(c.to_string()).width(Length::Fill).align_x(Alignment::Center))
                .class(crate::theme::letter_button(current == Some(c)))
                .padding([1, 0])
                .width(Length::Fixed(26.0))
                .on_press(Message::Library(LibraryMsg::Jump(c)))
                .into()
        });
        widget::container(widget::column::with_children(letters.collect::<Vec<_>>()).spacing(1))
            .center_y(Length::Fill)
            .padding([0, 8, 0, 0])
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_fills_the_width() {
        for width in [400.0, 1280.0, 1600.0, 1829.0, 2560.0] {
            let g = Grid::at(width);
            let used = g.cols as f32 * (g.poster.0 + 2.0 * CARD_PADDING) + (g.cols - 1) as f32 * COL_GAP;
            let avail = width - PAD_LEFT - PAD_RIGHT;
            assert!(used <= avail, "{width}: {used} > {avail}");
            // Less than a pixel per column left over: no gap at the side.
            assert!(avail - used < g.cols as f32 + 1.0, "{width}: {used} of {avail}");
            assert!(g.poster.0 >= MIN_POSTER_W || g.cols == 1);
        }
        // The owner's screenshot: 1829 px fitted 9 fixed cards; 10 now.
        assert_eq!(Grid::at(1829.0 - 50.0).cols, 10);
    }

    #[test]
    fn only_rows_on_screen_are_built() {
        let g = Grid::at(1600.0);
        let total = 4000;
        let v = g.visible(total, 0.0, 900.0);
        assert_eq!(v.start, 0);
        assert!(v.len() < 10);
        let far = g.visible(total, g.row_h * 100.0, 900.0);
        assert_eq!(far.start, 100 - SPARE_ROWS);
        let end = g.visible(total, 1e9, 900.0);
        assert!(end.is_empty() && end.end == g.rows(total));
    }

    #[test]
    fn letters_skip_articles() {
        assert_eq!(letter_of("The Matrix"), 'M');
        assert_eq!(letter_of("A Quiet Place"), 'Q');
        assert_eq!(letter_of("Akira"), 'A');
        assert_eq!(letter_of("2012"), '#');
        assert_eq!(letter_of("Être et avoir"), '#');
    }
}
