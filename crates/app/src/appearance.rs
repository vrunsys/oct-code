//! Light or dark: follow the OS unless the user pinned one.
//!
//! The pin comes from `AGENT_DESK_APPEARANCE` (`light`, `dark` or `system`).
//! It exists for desktops whose portal does not report a color scheme, and
//! for checking both themes without changing system settings.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Window};

pub const APPEARANCE_ENV: &str = "AGENT_DESK_APPEARANCE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    /// Reads [`APPEARANCE_ENV`]; anything unrecognized follows the system.
    pub fn from_env() -> Self {
        Self::parse(std::env::var(APPEARANCE_ENV).ok().as_deref())
    }

    pub fn parse(value: Option<&str>) -> Self {
        match value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            _ => Self::System,
        }
    }

    /// Applies this appearance to the theme. Call again when the window's
    /// system appearance changes; a pinned appearance ignores that change.
    pub fn apply(self, window: &mut Window, cx: &mut App) {
        match self {
            Self::System => Theme::sync_system_appearance(Some(window), cx),
            Self::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            Self::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Appearance;

    #[test]
    fn parses_light_dark_and_system() {
        assert_eq!(Appearance::parse(Some("dark")), Appearance::Dark);
        assert_eq!(Appearance::parse(Some(" Light ")), Appearance::Light);
        assert_eq!(Appearance::parse(Some("system")), Appearance::System);
    }

    #[test]
    fn missing_or_unknown_follows_the_system() {
        assert_eq!(Appearance::parse(None), Appearance::System);
        assert_eq!(Appearance::parse(Some("sepia")), Appearance::System);
    }
}
