//! Applying the light/dark preference to the window.
//!
//! The preference itself is [`Appearance`] in the saved settings.
//! `AGENT_DESK_APPEARANCE` (`light`, `dark` or `system`) overrides it for one
//! run, for desktops whose portal does not report a color scheme and for
//! checking both themes without changing system settings.

use agent_desk_core::settings::Appearance;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Window};

pub const APPEARANCE_ENV: &str = "AGENT_DESK_APPEARANCE";

/// The override from [`APPEARANCE_ENV`], if it names a known appearance.
pub fn appearance_from_env() -> Option<Appearance> {
    parse_appearance(std::env::var(APPEARANCE_ENV).ok().as_deref())
}

pub fn parse_appearance(value: Option<&str>) -> Option<Appearance> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "system" => Some(Appearance::System),
        "light" => Some(Appearance::Light),
        "dark" => Some(Appearance::Dark),
        _ => None,
    }
}

/// Applies `appearance` to the theme. Call again when the window's system
/// appearance changes; a pinned light or dark ignores that change.
pub fn apply_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
    match appearance {
        Appearance::System => Theme::sync_system_appearance(Some(window), cx),
        Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
    }
}

#[cfg(test)]
mod tests {
    use agent_desk_core::settings::Appearance;

    use super::parse_appearance;

    #[test]
    fn parses_light_dark_and_system() {
        assert_eq!(parse_appearance(Some("dark")), Some(Appearance::Dark));
        assert_eq!(parse_appearance(Some(" Light ")), Some(Appearance::Light));
        assert_eq!(parse_appearance(Some("system")), Some(Appearance::System));
    }

    #[test]
    fn missing_or_unknown_is_no_override() {
        assert_eq!(parse_appearance(None), None);
        assert_eq!(parse_appearance(Some("sepia")), None);
    }
}
