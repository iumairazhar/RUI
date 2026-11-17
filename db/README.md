# Database

The daemon owns the schema. On start it opens the database (creating it if missing) and applies any pending files from `db/migrations/` using `rusqlite_migration`. Migration N leaves `PRAGMA user_version` at N.

Before migrating an existing database, the daemon copies it to `<db>.bak-v<old>-<unix_secs>` and logs the backup path. A database newer than the daemon is refused.

`db/schema.sql` is a reference copy of the current schema only. A test checks it matches the migrations.

## Add a migration

1. Create `db/migrations/NNN_name.sql` (next number). No `BEGIN`/`COMMIT` and no `PRAGMA user_version`; the crate handles both.
2. Add one `include_str!` line to `MIGRATION_SQL` in `crates/rui-daemon/src/db.rs` (and bump the array length).
3. Bump `SCHEMA_VERSION` in `crates/rui-core/src/lib.rs`; a test fails if it drifts from the migration count.
4. Update `db/schema.sql` to match.
