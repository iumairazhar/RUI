//! Embedded icon assets.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

/// Serves `icons/<name>.svg` from bytes compiled into the binary.
pub struct Assets;

fn icon_bytes(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "icons/rui.svg" => include_bytes!("../assets/icons/rui.svg").as_slice(),
        "icons/threads.svg" => include_bytes!("../assets/icons/threads.svg"),
        "icons/inbox.svg" => include_bytes!("../assets/icons/inbox.svg"),
        "icons/work.svg" => include_bytes!("../assets/icons/work.svg"),
        "icons/seats.svg" => include_bytes!("../assets/icons/seats.svg"),
        "icons/hand.svg" => include_bytes!("../assets/icons/hand.svg"),
        "icons/check.svg" => include_bytes!("../assets/icons/check.svg"),
        "icons/send.svg" => include_bytes!("../assets/icons/send.svg"),
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
        for p in ["rui", "threads", "inbox", "work", "seats", "hand", "check", "send"] {
            let path = format!("icons/{p}.svg");
            assert!(Assets.load(&path).unwrap().is_some(), "{path}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }
}
