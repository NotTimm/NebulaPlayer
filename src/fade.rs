//! Fades its content out to the page's background, or in from it: the
//! home screen's featured banner between items.
//!
//! The fade is drawn, not rebuilt: the widget lays the page's colour over
//! its content at an opacity worked out from the clock on each frame, and
//! asks for the next frame until it's done. The app only rebuilds the page
//! when a fade starts and when the banner's item changes in the middle.

use std::time::{Duration, Instant};

use cosmic::iced::advanced::widget::{tree, Operation, Tree};
use cosmic::iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
use cosmic::iced::{window, Background, Color, Event, Length, Rectangle, Size, Vector};
use cosmic::Element;

/// How long fading out (or in) takes.
pub const DURATION: Duration = Duration::from_millis(260);

/// Which way, since when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Out(Instant),
    In(Instant),
}

impl Phase {
    /// How much of the page's colour covers the content at `now`: 0 to 1.
    pub fn cover(self, now: Instant) -> f32 {
        let (from, out) = match self {
            Phase::Out(t) => (t, true),
            Phase::In(t) => (t, false),
        };
        let t = (now.saturating_duration_since(from).as_secs_f32() / DURATION.as_secs_f32()).min(1.0);
        // Ease in and out.
        let eased = t * t * (3.0 - 2.0 * t);
        if out {
            eased
        } else {
            1.0 - eased
        }
    }

    fn done(self, now: Instant) -> bool {
        let from = match self {
            Phase::Out(t) | Phase::In(t) => t,
        };
        now.saturating_duration_since(from) >= DURATION
    }
}

pub struct Fade<'a, Message> {
    content: Element<'a, Message>,
    phase: Option<Phase>,
}

pub fn fade<'a, Message>(content: impl Into<Element<'a, Message>>, phase: Option<Phase>) -> Fade<'a, Message> {
    Fade { content: content.into(), phase }
}

impl<'a, Message> Widget<Message, cosmic::Theme, cosmic::Renderer> for Fade<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &cosmic::Renderer, limits: &layout::Limits) -> layout::Node {
        self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &cosmic::Renderer, operation: &mut dyn Operation) {
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &cosmic::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let (Event::Window(window::Event::RedrawRequested(now)), Some(phase)) = (event, self.phase) {
            if !phase.done(*now) {
                shell.request_redraw();
            }
        }
        self.content
            .as_widget_mut()
            .update(&mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &cosmic::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
        let cover = self.phase.map_or(0.0, |p| p.cover(Instant::now()));
        if cover > 0.0 {
            use cosmic::iced::advanced::Renderer as _;
            let bounds = layout.bounds();
            let base: Color = theme.cosmic().background(theme.transparent).base.into();
            renderer.with_layer(bounds, |renderer| {
                renderer.fill_quad(
                    renderer::Quad { bounds, ..Default::default() },
                    Background::Color(Color { a: base.a * cover, ..base }),
                );
            });
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &cosmic::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, cosmic::Theme, cosmic::Renderer>> {
        self.content.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a> From<Fade<'a, Message>> for Element<'a, Message> {
    fn from(f: Fade<'a, Message>) -> Self {
        Element::new(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_then_uncovers() {
        let t0 = Instant::now();
        assert_eq!(Phase::Out(t0).cover(t0), 0.0);
        assert_eq!(Phase::Out(t0).cover(t0 + DURATION), 1.0);
        assert_eq!(Phase::In(t0).cover(t0), 1.0);
        assert_eq!(Phase::In(t0).cover(t0 + DURATION * 2), 0.0);
        let mid = Phase::Out(t0).cover(t0 + DURATION / 2);
        assert!((mid - 0.5).abs() < 0.01, "{mid}");
    }
}
