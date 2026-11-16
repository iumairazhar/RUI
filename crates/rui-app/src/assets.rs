//! Embedded icon assets.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

/// Serves `icons/<name>.svg` from bytes compiled into the binary.
pub struct Assets;

fn icon_bytes(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "icons/rui.svg" => include_bytes!("../assets/icons/rui.svg").as_slice(),
        "icons/threads.svg" => include_bytes!("../assets/icons/threads.svg"),
        "icons/apps.svg" => include_bytes!("../assets/icons/apps.svg"),
        "icons/skills.svg" => include_bytes!("../assets/icons/skills.svg"),
        "icons/inbox.svg" => include_bytes!("../assets/icons/inbox.svg"),
        "icons/work.svg" => include_bytes!("../assets/icons/work.svg"),
        "icons/seats.svg" => include_bytes!("../assets/icons/seats.svg"),
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
        for p in ["rui", "threads", "apps", "skills", "inbox", "work", "seats", "hand", "check", "send", "file", "brain", "diff", "term", "you", "close", "plus", "chevron"] {
            let path = format!("icons/{p}.svg");
            assert!(Assets.load(&path).unwrap().is_some(), "{path}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }
}
