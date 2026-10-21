//! Jellyfin's look: near-black surfaces, its blue (#00A4DC) as the accent
//! and the purple-to-blue of its logo on the big Play buttons. Or, as a
//! setting, the desktop's own COSMIC theme.

use std::sync::Arc;

use cosmic::cosmic_theme::palette::{Srgb, Srgba};
use cosmic::cosmic_theme::ThemeBuilder;
use cosmic::iced::gradient::Linear;
use cosmic::iced::{Background, Border, Color, Gradient, Radians};
use cosmic::widget;

use crate::settings::ThemeChoice;

/// Jellyfin's blue, its main accent.
pub const BLUE: u32 = 0x00a4dc;
/// The purple end of Jellyfin's logo gradient.
pub const PURPLE: u32 = 0xaa5cc3;
/// Jellyfin web's dark theme background.
const BACKGROUND: u32 = 0x101010;
/// Its cards, header and drawers.
const SURFACE: u32 = 0x1c1c1c;
const TEXT: u32 = 0xeeeeee;

fn rgb(hex: u32) -> Srgb {
    Srgb::new(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    )
}

fn opaque(c: Srgb) -> Srgba {
    Srgba::new(c.red, c.green, c.blue, 1.0)
}

pub fn color(hex: u32, alpha: f32) -> Color {
    let c = rgb(hex);
    Color::from_rgba(c.red, c.green, c.blue, alpha)
}

pub fn build(choice: ThemeChoice) -> cosmic::Theme {
    match choice {
        ThemeChoice::System => cosmic::theme::system_preference(),
        ThemeChoice::Jellyfin => {
            let built = ThemeBuilder::dark()
                .accent(rgb(BLUE))
                .bg_color(opaque(rgb(BACKGROUND)))
                .primary_container_bg(opaque(rgb(SURFACE)))
                .text_tint(rgb(TEXT))
                .build();
            cosmic::Theme::custom(Arc::new(built))
        }
    }
}

/// Jellyfin's logo gradient, purple to blue, left to right.
pub fn brand_gradient(alpha: f32) -> Background {
    Background::Gradient(Gradient::Linear(
        Linear::new(Radians(std::f32::consts::FRAC_PI_2))
            .add_stop(0.0, color(PURPLE, alpha))
            .add_stop(1.0, color(BLUE, alpha)),
    ))
}

/// The big Play button: the brand gradient, brighter when hovered.
pub fn play_button() -> cosmic::theme::Button {
    fn style(alpha: f32) -> widget::button::Style {
        widget::button::Style {
            background: Some(brand_gradient(alpha)),
            border_radius: 20.0.into(),
            text_color: Some(Color::WHITE),
            icon_color: Some(Color::WHITE),
            ..widget::button::Style::new()
        }
    }
    cosmic::theme::Button::Custom {
        active: Box::new(|_, _| style(0.9)),
        disabled: Box::new(|_| style(0.4)),
        hovered: Box::new(|_, _| style(1.0)),
        pressed: Box::new(|_, _| style(0.75)),
    }
}

/// A darkening fade over the bottom of a backdrop, into the page's
/// background, so text on it stays readable.
pub fn backdrop_fade(theme: &cosmic::Theme) -> widget::container::Style {
    let bg: Color = theme.cosmic().bg_color().into();
    widget::container::Style {
        background: Some(Background::Gradient(Gradient::Linear(
            Linear::new(Radians(std::f32::consts::PI))
                .add_stop(0.0, Color { a: 0.0, ..bg })
                .add_stop(0.55, Color { a: 0.55, ..bg })
                .add_stop(1.0, bg),
        ))),
        ..Default::default()
    }
}

/// A darkening fade from the left, under the featured banner's text.
pub fn side_fade(theme: &cosmic::Theme) -> widget::container::Style {
    let bg: Color = theme.cosmic().bg_color().into();
    widget::container::Style {
        background: Some(Background::Gradient(Gradient::Linear(
            Linear::new(Radians(std::f32::consts::FRAC_PI_2))
                .add_stop(0.0, Color { a: 0.85, ..bg })
                .add_stop(0.5, Color { a: 0.35, ..bg })
                .add_stop(1.0, Color { a: 0.0, ..bg }),
        ))),
        ..Default::default()
    }
}

/// A card's placeholder while its picture loads (or when it has none).
pub fn placeholder(theme: &cosmic::Theme) -> widget::container::Style {
    let c = theme.cosmic();
    widget::container::Style {
        background: Some(Background::Color(c.primary_container_color().into())),
        icon_color: Some(c.on_primary_container_color().into()),
        text_color: Some(c.on_primary_container_color().into()),
        border: Border { radius: 6.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// A small dark pill on top of a picture (a "watched" tick, a count).
pub fn badge(theme: &cosmic::Theme) -> widget::container::Style {
    let _ = theme;
    widget::container::Style {
        background: Some(brand_gradient(0.95)),
        text_color: Some(Color::WHITE),
        icon_color: Some(Color::WHITE),
        border: Border { radius: 10.0.into(), ..Default::default() },
        ..Default::default()
    }
}
