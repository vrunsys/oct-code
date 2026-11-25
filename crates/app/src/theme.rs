//! The Agent Desk color theme.
//!
//! The palette lives in `themes/agent-desk.json`: ink `#212121` is the
//! primary, coral `#FF7759` the accent (focus ring, caret, selection), and
//! paper `#FAFAFA` the light surface. That file is the only place the app
//! names raw colors; views read semantic roles from `cx.theme()`.

use std::rc::Rc;

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeSet};

const BRAND_THEME_JSON: &str = include_str!("../themes/agent-desk.json");

/// The bundled light and dark themes.
pub fn brand_themes() -> (ThemeConfig, ThemeConfig) {
    // Bundled at compile time and covered by tests, so a parse failure is a
    // build defect, not a runtime condition.
    let set: ThemeSet =
        serde_json::from_str(BRAND_THEME_JSON).expect("themes/agent-desk.json is valid");
    let pick = |mode: ThemeMode| {
        set.themes
            .iter()
            .find(|theme| theme.mode == mode)
            .cloned()
            .unwrap_or_else(|| panic!("themes/agent-desk.json has a {mode:?} theme"))
    };
    (pick(ThemeMode::Light), pick(ThemeMode::Dark))
}

/// Makes the brand themes the ones light and dark mode load. Call after
/// `gpui_kit::init` and before applying an appearance.
pub fn install(cx: &mut App) {
    let (light, dark) = brand_themes();
    Theme::update(cx, |theme| {
        theme.light_theme = Rc::new(light);
        theme.dark_theme = Rc::new(dark);
    });
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::{Theme, ThemeMode};
    use gpui_kit::{Hsla, TestAppContext, px, rgb};

    use super::{brand_themes, install};

    /// WCAG relative luminance of an sRGB color.
    fn luminance(color: Hsla) -> f32 {
        let rgba = color.to_rgb();
        let channel = |c: f32| {
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgba.r) + 0.7152 * channel(rgba.g) + 0.0722 * channel(rgba.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (hi, lo) = {
            let (la, lb) = (luminance(a), luminance(b));
            if la > lb { (la, lb) } else { (lb, la) }
        };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn brand_file_has_a_light_and_a_dark_theme() {
        let (light, dark) = brand_themes();
        assert_eq!(light.mode, ThemeMode::Light);
        assert_eq!(dark.mode, ThemeMode::Dark);
    }

    #[gpui_kit::test]
    fn installed_theme_uses_the_brand_colors(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
            Theme::change(ThemeMode::Light, None, cx);
            let theme = Theme::global(cx);
            assert_eq!(theme.background, Hsla::from(rgb(0xFAFAFA)));
            assert_eq!(theme.foreground, Hsla::from(rgb(0x212121)));
            assert_eq!(theme.primary, Hsla::from(rgb(0x212121)));
            assert_eq!(theme.ring, Hsla::from(rgb(0xFF7759)));

            Theme::change(ThemeMode::Dark, None, cx);
            let theme = Theme::global(cx);
            assert_eq!(theme.background, Hsla::from(rgb(0x212121)));
            assert_eq!(theme.foreground, Hsla::from(rgb(0xFAFAFA)));
            assert_eq!(theme.ring, Hsla::from(rgb(0xFF7759)));
        });
    }

    #[gpui_kit::test]
    fn corners_are_square_in_both_modes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
            for mode in [ThemeMode::Light, ThemeMode::Dark] {
                Theme::change(mode, None, cx);
                let theme = Theme::global(cx);
                assert_eq!(theme.radius, px(0.), "{mode:?} radius");
                assert_eq!(theme.radius_lg, px(0.), "{mode:?} large radius");
            }
        });
    }

    #[gpui_kit::test]
    fn text_is_readable_in_both_modes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
            for mode in [ThemeMode::Light, ThemeMode::Dark] {
                Theme::change(mode, None, cx);
                let t = Theme::global(cx);
                let pairs = [
                    ("foreground on background", t.foreground, t.background),
                    ("muted text on background", t.muted_foreground, t.background),
                    ("muted text on muted", t.muted_foreground, t.muted),
                    ("primary text on primary", t.primary_foreground, t.primary),
                    ("sidebar text on sidebar", t.sidebar_foreground, t.sidebar),
                    ("popover text on popover", t.popover_foreground, t.popover),
                ];
                for (name, text, surface) in pairs {
                    let ratio = contrast(text, surface);
                    assert!(
                        ratio >= 4.5,
                        "{mode:?}: {name} is {ratio:.2}:1, below 4.5:1"
                    );
                }
            }
        });
    }
}
