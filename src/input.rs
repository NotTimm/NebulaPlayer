//! A wrapper around the page that makes scrolling feel like a browser's,
//! and can catch any click inside it (to close the sidebar).
//!
//! Scrolling: iced moves a scrollable exactly as far as each wheel event
//! says, and stops dead. Here a mouse wheel's steps glide to where they
//! lead (a short ease-out, like Chrome's smooth scrolling), and a touchpad
//! swipe keeps going after the fingers lift, slowing down (a fling). Both
//! are done by feeding the content small pixel scroll events on each
//! frame, so every scrollable inside works unchanged, and iced's own
//! latching keeps a glide on the scrollable it started on.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cosmic::iced::advanced::widget::{tree, Operation, Tree};
use cosmic::iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
use cosmic::iced::{keyboard, touch, window, Event, Length, Point, Rectangle, Size, Vector};
use cosmic::Element;

/// Pixels one wheel step scrolls (iced's own is 60; browsers use ~100).
const LINE: f32 = 90.0;
/// How quickly a wheel step's glide settles: the time constant of its
/// ease-out.
const WHEEL_EASE: f32 = 0.075;
/// How quickly a fling slows: speed falls by e every this many seconds,
/// so a fling travels its starting speed × this far.
const FLING_DECAY: f32 = 0.33;
/// No touchpad event for this long: the fingers have lifted.
const LIFT_GAP: Duration = Duration::from_millis(60);
/// The fling's speed comes from the motion over this last stretch.
const VELOCITY_WINDOW: Duration = Duration::from_millis(100);
/// Slower than this (pixels per second) and a fling doesn't start or stops.
const MIN_FLING: f32 = 180.0;
const MAX_FLING: f32 = 9000.0;
const STOP_SPEED: f32 = 25.0;

#[derive(Debug, Default)]
enum Motion {
    #[default]
    Idle,
    /// Touchpad events arriving: they pass straight through, and are
    /// sampled for the fling.
    Tracking,
    /// Coasting after a touchpad swipe, in pixels per second.
    Fling { velocity: Vector, last: Instant },
    /// Gliding through wheel steps: how far is still to go.
    Wheel { remaining: Vector, last: Instant },
}

#[derive(Debug, Default)]
struct State {
    motion: Motion,
    /// Recent touchpad deltas.
    samples: VecDeque<(Instant, Vector)>,
    /// Where the pointer was when the scroll started; synthetic events go
    /// there, so the glide stays on what was scrolled.
    anchor: Point,
    shift: bool,
}

pub struct Gestures<'a, Message> {
    content: Element<'a, Message>,
    on_press: Option<Message>,
}

/// Wraps `content` in browser-like scrolling.
pub fn gestures<'a, Message>(content: impl Into<Element<'a, Message>>) -> Gestures<'a, Message> {
    Gestures { content: content.into(), on_press: None }
}

impl<'a, Message> Gestures<'a, Message> {
    /// Sends `message` on any mouse press or touch inside, instead of
    /// letting the content have it.
    pub fn on_press_maybe(mut self, message: Option<Message>) -> Self {
        self.on_press = message;
        self
    }
}

/// A wheel event's movement in pixels, the way scrollables read it.
fn pixels(delta: &mouse::ScrollDelta, shift: bool) -> Vector {
    match *delta {
        mouse::ScrollDelta::Lines { x, y } => {
            // Shift turns the wheel sideways (macOS does that itself).
            let (x, y) = if shift && !cfg!(target_os = "macos") { (y, x) } else { (x, y) };
            Vector::new(x * LINE, y * LINE)
        }
        mouse::ScrollDelta::Pixels { x, y } => Vector::new(x, y),
    }
}

fn length(v: Vector) -> f32 {
    (v.x * v.x + v.y * v.y).sqrt()
}

impl State {
    /// The touchpad's speed over its last few events, in pixels per second.
    fn release_velocity(&self) -> Vector {
        let Some(&(last, _)) = self.samples.back() else { return Vector::ZERO };
        let recent: Vec<_> = self.samples.iter().filter(|(t, _)| last.duration_since(*t) <= VELOCITY_WINDOW).collect();
        let first = recent.first().map_or(last, |(t, _)| *t);
        // Each event is the motion since the one before, so the stretch
        // covered is one event interval longer than first-to-last.
        let interval = if recent.len() > 1 {
            last.duration_since(first).as_secs_f32() / (recent.len() - 1) as f32
        } else {
            1.0 / 60.0
        };
        let span = last.duration_since(first).as_secs_f32() + interval;
        let sum = recent.iter().fold(Vector::ZERO, |acc, (_, d)| acc + *d);
        let v = Vector::new(sum.x / span, sum.y / span);
        let speed = length(v);
        if speed > MAX_FLING {
            v * (MAX_FLING / speed)
        } else {
            v
        }
    }
}

impl<'a, Message: Clone> Widget<Message, cosmic::Theme, cosmic::Renderer> for Gestures<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
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
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        let content = &mut tree.children[0];

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => state.shift = m.shift(),
            // Any press or key stops a glide, as in a browser.
            Event::Mouse(mouse::Event::ButtonPressed(_)) | Event::Touch(touch::Event::FingerPressed { .. }) => {
                state.motion = Motion::Idle;
                // The press only does that: the page moves as the sidebar
                // closes, so what's under the pointer isn't what was aimed at.
                if let (true, Some(msg)) = (cursor.is_over(bounds), &self.on_press) {
                    shell.publish(msg.clone());
                    shell.capture_event();
                    return;
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed { .. }) => state.motion = Motion::Idle,
            _ => {}
        }

        if let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event {
            if let Some(at) = cursor.position_over(bounds) {
                let now = Instant::now();
                match delta {
                    mouse::ScrollDelta::Pixels { .. } => {
                        // A touchpad: the content follows the fingers
                        // exactly; only what happens after they lift
                        // is ours.
                        if !matches!(state.motion, Motion::Tracking) {
                            state.samples.clear();
                            state.anchor = at;
                        }
                        state.motion = Motion::Tracking;
                        state.samples.push_back((now, pixels(delta, state.shift)));
                        while state.samples.len() > 32 {
                            state.samples.pop_front();
                        }
                        shell.request_redraw();
                    }
                    mouse::ScrollDelta::Lines { .. } => {
                        let step = pixels(delta, state.shift);
                        state.motion = match std::mem::take(&mut state.motion) {
                            // Steps while gliding add up, so a quick
                            // spin of the wheel goes further, faster.
                            Motion::Wheel { remaining, last } => Motion::Wheel { remaining: remaining + step, last },
                            _ => {
                                state.anchor = at;
                                Motion::Wheel { remaining: step, last: now }
                            }
                        };
                        shell.capture_event();
                        shell.request_redraw();
                        return;
                    }
                }
            }
        }

        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let now = *now;
            let mut step = Vector::ZERO;
            match &mut state.motion {
                Motion::Idle => {}
                Motion::Tracking => {
                    let last = state.samples.back().map_or(now, |(t, _)| *t);
                    if now.saturating_duration_since(last) >= LIFT_GAP {
                        let velocity = state.release_velocity();
                        state.samples.clear();
                        state.motion = if length(velocity) >= MIN_FLING {
                            Motion::Fling { velocity, last: now }
                        } else {
                            Motion::Idle
                        };
                    }
                    shell.request_redraw();
                }
                Motion::Fling { velocity, last } => {
                    let dt = now.saturating_duration_since(*last).as_secs_f32().min(0.05);
                    *last = now;
                    // The distance covered over dt with exponentially
                    // falling speed.
                    let decay = (-dt / FLING_DECAY).exp();
                    let travelled = FLING_DECAY * (1.0 - decay);
                    step = Vector::new(velocity.x * travelled, velocity.y * travelled);
                    *velocity = Vector::new(velocity.x * decay, velocity.y * decay);
                    if length(*velocity) < STOP_SPEED {
                        state.motion = Motion::Idle;
                    } else {
                        shell.request_redraw();
                    }
                }
                Motion::Wheel { remaining, last } => {
                    let dt = now.saturating_duration_since(*last).as_secs_f32().min(0.05);
                    *last = now;
                    let part = 1.0 - (-dt / WHEEL_EASE).exp();
                    step = Vector::new(remaining.x * part, remaining.y * part);
                    *remaining = *remaining - step;
                    if length(*remaining) < 0.5 {
                        step = step + *remaining;
                        state.motion = Motion::Idle;
                    } else {
                        shell.request_redraw();
                    }
                }
            }
            if step != Vector::ZERO {
                let scroll = Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Pixels { x: step.x, y: step.y },
                });
                let at = mouse::Cursor::Available(state.anchor);
                self.content
                    .as_widget_mut()
                    .update(content, &scroll, layout, at, renderer, clipboard, shell, viewport);
            }
        }

        self.content
            .as_widget_mut()
            .update(content, event, layout, cursor, renderer, clipboard, shell, viewport);
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

    fn drag_destinations(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        renderer: &cosmic::Renderer,
        dnd_rectangles: &mut cosmic::iced::advanced::clipboard::DndDestinationRectangles,
    ) {
        self.content.as_widget().drag_destinations(&tree.children[0], layout, renderer, dnd_rectangles);
    }
}

impl<'a, Message: Clone + 'a> From<Gestures<'a, Message>> for Element<'a, Message> {
    fn from(g: Gestures<'a, Message>) -> Self {
        Element::new(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_velocity_is_recent_motion_over_time() {
        let mut s = State::default();
        let t0 = Instant::now();
        // 10 px every 10 ms: 1000 px/s.
        for i in 0..20u64 {
            s.samples.push_back((t0 + Duration::from_millis(10 * i), Vector::new(0.0, 10.0)));
        }
        let v = s.release_velocity();
        assert!((v.y - 1000.0).abs() < 1.0, "{v:?}");
        assert_eq!(v.x, 0.0);
    }

    #[test]
    fn wheel_steps_turn_sideways_with_shift() {
        let d = mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 };
        assert_eq!(pixels(&d, false), Vector::new(0.0, -LINE));
        if !cfg!(target_os = "macos") {
            assert_eq!(pixels(&d, true), Vector::new(-LINE, 0.0));
        }
    }
}
