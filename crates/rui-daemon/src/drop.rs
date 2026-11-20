//! The drop folder `<data_dir>/drop/`: `.eml` and `.ics` files placed there are read on the
//! heartbeat. Each file is first renamed into `drop/processing/` (so it is never read twice), then
//! stored as events and moved to `drop/done/`, or to `drop/failed/` when it cannot be stored.

use crate::db::Db;
use crate::{email, meeting};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// The drop folder.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("drop")
}

/// Reads every dropped file once. Returns how many were handled; errors are logged per file.
pub fn scan(db: &Db, data_dir: &Path) -> Result<usize> {
    let d = dir(data_dir);
    if !d.is_dir() {
        return Ok(0);
    }
    let mut n = 0;
    for path in fs::read_dir(&d)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()) {
        match handle(db, data_dir, &path) {
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(e) => crate::scheduler::report_error(db, None, &e.context(format!("drop file {}", path.display()))),
        }
    }
    Ok(n)
}

/// Claims one file into processing/, stores it, then moves it to done/ or failed/.
/// False for files that are not .eml or .ics.
fn handle(db: &Db, data_dir: &Path, path: &Path) -> Result<bool> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    if ext != "eml" && ext != "ics" {
        return Ok(false);
    }
    let drop = dir(data_dir);
    let claimed = move_into(path, &drop.join("processing"))?;
    match ingest(db, data_dir, &claimed, &ext) {
        Ok(()) => move_into(&claimed, &drop.join("done")).map(|_| true),
        Err(e) => {
            move_into(&claimed, &drop.join("failed"))?;
            Err(e)
        }
    }
}

/// Reads one claimed file and stores its email or meetings.
fn ingest(db: &Db, data_dir: &Path, path: &Path, ext: &str) -> Result<()> {
    let raw = String::from_utf8_lossy(&fs::read(path)?).into_owned();
    if ext == "eml" {
        email::receive(db, data_dir, &email::parse_eml(&raw), None, "drop")?;
    }
    for m in if ext == "ics" { meeting::parse_ics(&raw) } else { vec![] } {
        meeting::receive(db, data_dir, &m, None, "drop")?;
    }
    Ok(())
}

/// Renames a file into `folder` (one atomic rename), prefixing a timestamp when the name is taken.
fn move_into(path: &Path, folder: &Path) -> Result<PathBuf> {
    fs::create_dir_all(folder)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut to = folder.join(&name);
    if to.exists() {
        to = folder.join(format!("{}-{name}", chrono::Utc::now().format("%Y%m%d%H%M%S%3f")));
    }
    fs::rename(path, &to)?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_state;

    fn setup() -> (Db, tempfile::TempDir, i64) {
        let db = Db::open_test();
        let d = tempfile::tempdir().unwrap();
        let c = db.create_client("acme", "/tmp/acme", None, &[]).unwrap();
        client_state::ensure(d.path(), "acme", "/tmp/acme", None).unwrap();
        let path = client_state::dir(d.path(), "acme").join("client.json");
        let with = fs::read_to_string(&path).unwrap().replace("\"internal\": false", "\"internal\": false, \"domains\": [\"acme.com\"]");
        fs::write(&path, with).unwrap();
        (db, d, c.id)
    }

    #[test]
    fn eml_and_ics_are_parsed_and_moved() {
        let (db, d, cid) = setup();
        assert_eq!(scan(&db, d.path()).unwrap(), 0);
        let drop = dir(d.path());
        fs::create_dir_all(&drop).unwrap();
        fs::write(drop.join("a.eml"), "From: Bob <bob@acme.com>\nSubject: Invoice\n\nPlease check.\n").unwrap();
        fs::write(drop.join("m.ics"), "BEGIN:VEVENT\nSUMMARY:Sync\nDTSTART:20261118T150000Z\nATTENDEE:mailto:bob@acme.com\nEND:VEVENT\n").unwrap();
        fs::write(drop.join("notes.txt"), "ignored").unwrap();
        assert_eq!(scan(&db, d.path()).unwrap(), 2);
        assert!(drop.join("done/a.eml").exists() && drop.join("done/m.ics").exists() && drop.join("notes.txt").exists());
        assert!(!drop.join("a.eml").exists());
        let mail = email::open_emails(&db).unwrap();
        assert_eq!((mail[0].payload["subject"].as_str(), mail[0].payload["client_id"].as_i64()), (Some("Invoice"), Some(cid)));
        assert_eq!(mail[0].payload["body"], "Please check.");
        let m = db.events_of_kinds(&["meeting_scheduled"]).unwrap();
        assert_eq!((m.len(), m[0].payload["client_id"].as_i64()), (1, Some(cid)));
        fs::write(drop.join("a.eml"), "From: x@y.z\n\nagain").unwrap();
        assert_eq!(scan(&db, d.path()).unwrap(), 1);
        assert_eq!(fs::read_dir(drop.join("done")).unwrap().count(), 3);
    }

    #[test]
    fn a_failed_file_goes_to_failed_and_is_never_read_again() {
        let (db, d, _) = setup();
        let drop = dir(d.path());
        fs::create_dir_all(&drop).unwrap();
        fs::write(drop.join("bad.eml"), "Subject: no sender\n\nbody").unwrap();
        assert_eq!(scan(&db, d.path()).unwrap(), 0);
        assert!(drop.join("failed/bad.eml").exists() && !drop.join("bad.eml").exists());
        assert!(fs::read_dir(drop.join("processing")).unwrap().next().is_none());
        assert_eq!(scan(&db, d.path()).unwrap(), 0);
        fs::write(drop.join("processing/left.eml"), "From: bob@acme.com\n\nhi").unwrap();
        assert_eq!(scan(&db, d.path()).unwrap(), 0, "a file left in processing is never re-ingested");
        assert!(email::open_emails(&db).unwrap().is_empty());
    }
}
