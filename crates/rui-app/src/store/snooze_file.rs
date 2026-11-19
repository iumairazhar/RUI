//! Snoozed Inbox items, saved to `snoozes.json` so they survive an app restart.

use super::ItemKey;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the snoozes live: next to the daemon's data.
pub fn path() -> PathBuf {
    crate::api::data_dir().join("snoozes.json")
}

/// Saved snoozes; none when the file is missing or unreadable.
pub fn load(path: &Path) -> HashMap<ItemKey, i64> {
    let Ok(text) = std::fs::read_to_string(path) else { return HashMap::new() };
    let rows: Vec<(String, i64, i64)> = serde_json::from_str(&text).unwrap_or_default();
    rows.into_iter().map(|(kind, id, until)| ((kind, id), until)).collect()
}

/// Write the snoozes that have not ended yet. A failed write only loses them on restart.
pub fn save(path: &Path, snoozed: &HashMap<ItemKey, i64>, now: i64) {
    let rows: Vec<(&str, i64, i64)> = snoozed.iter().filter(|(_, u)| **u > now).map(|((k, id), u)| (k.as_str(), *id, *u)).collect();
    if let Ok(text) = serde_json::to_string(&rows) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snoozes_round_trip_and_drop_ended_ones() {
        let dir = std::env::temp_dir().join(format!("rui-snooze-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("snoozes.json");
        let mut s = HashMap::new();
        s.insert(("question".to_string(), 7), 200);
        s.insert(("ask".to_string(), 3), 50);
        save(&file, &s, 100);
        let back = load(&file);
        assert_eq!(back.len(), 1);
        assert_eq!(back.get(&("question".to_string(), 7)), Some(&200));
        assert!(load(&dir.join("missing.json")).is_empty());
    }
}
