use std::path::PathBuf;

/// Folder for RUI's data: `RUI_DATA_DIR` if set, else ~/Library/Application Support/RUI.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RUI_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").expect("HOME is not set");
    PathBuf::from(home).join("Library/Application Support/RUI")
}

/// Path of the SQLite database file.
pub fn db_path() -> PathBuf {
    data_dir().join("rui.db")
}
