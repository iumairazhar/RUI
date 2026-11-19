//! Embedded icon and character assets.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

/// Serves `icons/<name>.svg` from bytes compiled into the binary.
pub struct Assets;

fn icon_bytes(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "icons/threads.svg" => include_bytes!("../assets/icons/threads.svg").as_slice(),
        "icons/lock.svg" => include_bytes!("../assets/icons/lock.svg"),
        "icons/hand.svg" => include_bytes!("../assets/icons/hand.svg"),
        "icons/check.svg" => include_bytes!("../assets/icons/check.svg"),
        "icons/send.svg" => include_bytes!("../assets/icons/send.svg"),
        "icons/file.svg" => include_bytes!("../assets/icons/file.svg"),
        "icons/brain.svg" => include_bytes!("../assets/icons/brain.svg"),
        "icons/diff.svg" => include_bytes!("../assets/icons/diff.svg"),
        "icons/term.svg" => include_bytes!("../assets/icons/term.svg"),
        "icons/you.svg" => include_bytes!("../assets/icons/you.svg"),
        "icons/plus.svg" => include_bytes!("../assets/icons/plus.svg"),
        "icons/chevron.svg" => include_bytes!("../assets/icons/chevron.svg"),
        "icons/close.svg" => include_bytes!("../assets/icons/close.svg"),
        "icons/nav-rui.svg" => include_bytes!("../assets/icons/nav-rui.svg"),
        "icons/nav-inbox.svg" => include_bytes!("../assets/icons/nav-inbox.svg"),
        "icons/nav-clients.svg" => include_bytes!("../assets/icons/nav-clients.svg"),
        "icons/nav-knowledge.svg" => include_bytes!("../assets/icons/nav-knowledge.svg"),
        "icons/nav-cost.svg" => include_bytes!("../assets/icons/nav-cost.svg"),
        "icons/nav-analytics.svg" => include_bytes!("../assets/icons/nav-analytics.svg"),
        "icons/nav-settings.svg" => include_bytes!("../assets/icons/nav-settings.svg"),
        "icons/nav-filter.svg" => include_bytes!("../assets/icons/nav-filter.svg"),
        "icons/moon.svg" => include_bytes!("../assets/icons/moon.svg"),
        "icons/arrow.svg" => include_bytes!("../assets/icons/arrow.svg"),
        "icons/claude.svg" => include_bytes!("../assets/icons/claude.svg"),
        "rui/shadow.svg" => include_bytes!("../assets/rui/shadow.svg"),
        "rui/stem.svg" => include_bytes!("../assets/rui/stem.svg"),
        "rui/bulb.svg" => include_bytes!("../assets/rui/bulb.svg"),
        "rui/shell.svg" => include_bytes!("../assets/rui/shell.svg"),
        "rui/visor.svg" => include_bytes!("../assets/rui/visor.svg"),
        "rui/eyes.svg" => include_bytes!("../assets/rui/eyes.svg"),
        "rui/eyes-wide.svg" => include_bytes!("../assets/rui/eyes-wide.svg"),
        "rui/eyes-happy.svg" => include_bytes!("../assets/rui/eyes-happy.svg"),
        "rui/mouth-flat.svg" => include_bytes!("../assets/rui/mouth-flat.svg"),
        "rui/mouth-smile.svg" => include_bytes!("../assets/rui/mouth-smile.svg"),
        "rui/mouth-o.svg" => include_bytes!("../assets/rui/mouth-o.svg"),
        "rui/mouth-wave.svg" => include_bytes!("../assets/rui/mouth-wave.svg"),
        "rui/band.svg" => include_bytes!("../assets/rui/band.svg"),
        "rui/think1.svg" => include_bytes!("../assets/rui/think1.svg"),
        "rui/think2.svg" => include_bytes!("../assets/rui/think2.svg"),
        "rui/spark.svg" => include_bytes!("../assets/rui/spark.svg"),
        "rui/ping.svg" => include_bytes!("../assets/rui/ping.svg"),
        _ => return None,
    })
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_icons_load() {
        for p in ["nav-rui", "nav-inbox", "nav-clients", "nav-knowledge", "nav-cost", "nav-analytics", "nav-settings", "nav-filter", "claude", "threads", "lock", "hand", "check", "send", "file", "brain", "diff", "term", "you", "close", "plus", "chevron"] {
            let path = format!("icons/{p}.svg");
            assert!(Assets.load(&path).unwrap().is_some(), "{path}");
        }
        for p in crate::views::character::LAYERS {
            let path = format!("rui/{p}.svg");
            assert!(Assets.load(&path).unwrap().is_some(), "{path}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }
}
