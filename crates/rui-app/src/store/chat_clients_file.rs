//! The client picked for each chat (chat id to client id), saved to `chat_clients.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where the picks live: next to the daemon's data.
pub fn path() -> PathBuf {
    crate::api::data_dir().join("chat_clients.json")
}

/// Saved picks; none when the file is missing or unreadable.
pub fn load(path: &Path) -> HashMap<i64, i64> {
    let Ok(text) = std::fs::read_to_string(path) else { return HashMap::new() };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Write the picks. A failed write only loses them on restart.
pub fn save(path: &Path, picks: &HashMap<i64, i64>) {
    if let Ok(text) = serde_json::to_string(picks) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_round_trip() {
        let dir = std::env::temp_dir().join(format!("rui-chat-clients-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("chat_clients.json");
        let picks = HashMap::from([(4, 9), (0, 2)]);
        save(&file, &picks);
        assert_eq!(load(&file), picks);
        assert!(load(&dir.join("missing.json")).is_empty());
    }
}
