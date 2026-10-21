//! Jellyfin's look: near-black surfaces, its blue (#00A4DC) as the accent
//! and the purple-to-blue of its logo on the big Play buttons. The accent
//! follows the desktop's when it has one (COSMIC, KDE, GNOME 47+), unless
//! the user picks Jellyfin's blue. Or, as a setting, the desktop's own
//! COSMIC theme.

use std::sync::Arc;

use cosmic::cosmic_theme::palette::{FromColor, Hsl, ShiftHue, Srgb, Srgba};
use cosmic::cosmic_theme::ThemeBuilder;
use cosmic::iced::gradient::Linear;
use cosmic::iced::{Background, Border, Color, Gradient, Radians};
use cosmic::widget;

use crate::settings::{AccentChoice, ThemeChoice};

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

/// The app's theme. `desktop_accent` is the desktop's accent colour, when
/// it has one; the Jellyfin theme uses it unless told not to.
pub fn build(choice: ThemeChoice, accent: AccentChoice, desktop_accent: Option<Srgb>) -> cosmic::Theme {
    match choice {
        ThemeChoice::System => cosmic::theme::system_preference(),
        ThemeChoice::Jellyfin => {
            let accent = match accent {
                AccentChoice::Desktop => desktop_accent.unwrap_or(rgb(BLUE)),
                AccentChoice::Jellyfin => rgb(BLUE),
            };
            let built = ThemeBuilder::dark()
                .accent(accent)
                .bg_color(opaque(rgb(BACKGROUND)))
                .primary_container_bg(opaque(rgb(SURFACE)))
                .text_tint(rgb(TEXT))
                .build();
            cosmic::Theme::custom(Arc::new(built))
        }
    }
}

/// The desktop's accent colour, read straight from COSMIC's settings so
/// the first frame already has it. Other desktops' accents arrive a moment
/// later from the settings portal (see `accent_changes`).
pub fn desktop_accent() -> Option<Srgb> {
    #[cfg(target_os = "linux")]
    if std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_uppercase().contains("COSMIC")) {
        let a = cosmic::theme::system_preference().cosmic().accent_color();
        return Some(Srgb::new(a.red, a.green, a.blue));
    }
    None
}

/// The desktop's accent colour from the XDG settings portal, now and
/// whenever it changes.
#[cfg(target_os = "linux")]
pub fn accent_changes() -> cosmic::iced::Subscription<Srgb> {
    use cosmic::iced::futures::{SinkExt, StreamExt};
    cosmic::iced::Subscription::run(|| {
        cosmic::iced::stream::channel(4, |mut tx: cosmic::iced::futures::channel::mpsc::Sender<Srgb>| async move {
            let to_srgb = |c: ashpd::desktop::Color| Srgb::new(c.red() as f32, c.green() as f32, c.blue() as f32);
            let Ok(settings) = ashpd::desktop::settings::Settings::new().await else { return };
            match settings.accent_color().await {
                Ok(c) => {
                    let _ = tx.send(to_srgb(c)).await;
                }
                // Not every desktop has one (GNOME before 47).
                Err(e) => log::debug!("no accent colour from the portal: {e}"),
            }
            if let Ok(mut changes) = settings.receive_accent_color_changed().await {
                while let Some(c) = changes.next().await {
                    let _ = tx.send(to_srgb(c)).await;
                }
            }
        })
    })
}

/// The two ends of the brand gradient: Jellyfin's purple to blue, or with
/// another accent, that accent and a neighbouring hue.
fn brand_colors(theme: &cosmic::Theme, alpha: f32) -> (Color, Color) {
    let a = theme.cosmic().accent_color();
    let accent = Srgb::new(a.red, a.green, a.blue);
    let blue = rgb(BLUE);
    let near = |x: f32, y: f32| (x - y).abs() < 0.02;
    if near(accent.red, blue.red) && near(accent.green, blue.green) && near(accent.blue, blue.blue) {
        return (color(PURPLE, alpha), color(BLUE, alpha));
    }
    let start = Srgb::from_color(Hsl::from_color(accent).shift_hue(50.0));
    (
        Color::from_rgba(start.red, start.green, start.blue, alpha),
        Color::from_rgba(accent.red, accent.green, accent.blue, alpha),
    )
}

/// The logo gradient (or the accent's own), left to right.
pub fn brand_gradient(theme: &cosmic::Theme, alpha: f32) -> Background {
    let (start, end) = brand_colors(theme, alpha);
    Background::Gradient(Gradient::Linear(
        Linear::new(Radians(std::f32::consts::FRAC_PI_2))
            .add_stop(0.0, start)
            .add_stop(1.0, end),
    ))
}

/// The big Play button: the brand gradient, brighter when hovered.
pub fn play_button() -> cosmic::theme::Button {
    fn style(theme: &cosmic::Theme, alpha: f32) -> widget::button::Style {
        widget::button::Style {
            background: Some(brand_gradient(theme, alpha)),
            border_radius: 20.0.into(),
            text_color: Some(Color::WHITE),
            icon_color: Some(Color::WHITE),
            ..widget::button::Style::new()
        }
    }
    cosmic::theme::Button::Custom {
        active: Box::new(|_, t| style(t, 0.9)),
        disabled: Box::new(|t| style(t, 0.4)),
        hovered: Box::new(|_, t| style(t, 1.0)),
        pressed: Box::new(|_, t| style(t, 0.75)),
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
    widget::container::Style {
        background: Some(brand_gradient(theme, 0.95)),
        text_color: Some(Color::WHITE),
        icon_color: Some(Color::WHITE),
        border: Border { radius: 10.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// A clickable card: plain text colours, a soft highlight on hover.
pub fn card_button() -> cosmic::theme::Button {
    fn style(theme: &cosmic::Theme, highlight: f32) -> widget::button::Style {
        let c = theme.cosmic();
        let on_bg: Color = c.on_bg_color().into();
        widget::button::Style {
            background: (highlight > 0.0).then(|| Background::Color(Color { a: highlight, ..on_bg })),
            border_radius: 8.0.into(),
            text_color: Some(on_bg),
            icon_color: Some(on_bg),
            ..widget::button::Style::new()
        }
    }
    cosmic::theme::Button::Custom {
        active: Box::new(|_, t| style(t, 0.0)),
        disabled: Box::new(|t| style(t, 0.0)),
        hovered: Box::new(|_, t| style(t, 0.08)),
        pressed: Box::new(|_, t| style(t, 0.14)),
    }
}
