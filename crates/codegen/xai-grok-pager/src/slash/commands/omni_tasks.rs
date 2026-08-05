//! `/omni-tasks` — list tasks from the omnitrix core SQLite store.
//!
//! Reads the `tasks` table (omni-storage schema, migration 0003) directly from
//! `dirs::data_dir()/omnitrix/omnitrix.sqlite`. No task rows, a missing
//! database, or a missing table all render as a "no tasks" message — never a
//! panic (I6: no unwrap/expect in this code path).

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// List tasks recorded by the omnitrix core.
pub struct OmniTasksCommand;

impl OmniTasksCommand {
    pub fn new() -> Self {
        Self
    }
}

/// Path of the omnitrix core SQLite store, or `None` when the platform
/// provides no data directory.
fn omnitrix_db_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("omnitrix").join("omnitrix.sqlite"))
}

/// Build the tasks listing message. Every failure mode returns an error
/// message or the "no tasks" message — no panics.
fn list_tasks() -> String {
    let path = match omnitrix_db_path() {
        Some(p) => p,
        None => return "gorev yok (data dizini bulunamadi)".to_string(),
    };

    let conn = match rusqlite::Connection::open(&path) {
        Ok(c) => c,
        Err(e) => return format!("gorev yok (veritabani acilamadi: {e})"),
    };

    let query = "SELECT id, title, mode, status, created_at FROM tasks ORDER BY id";
    let mut stmt = match conn.prepare(query) {
        Ok(s) => s,
        Err(e) if e.to_string().contains("no such table") => {
            return "gorev yok".to_string();
        }
        Err(e) => return format!("gorev yok (sorgu hatasi: {e})"),
    };

    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    }) {
        Ok(r) => r,
        Err(e) => return format!("gorev yok (sorgu hatasi: {e})"),
    };

    let mut lines: Vec<String> = Vec::new();
    for row in rows {
        match row {
            Ok((id, title, mode, status, created_at)) => {
                lines.push(format!("#{id} [{status}] {mode} - {title} ({created_at})"));
            }
            Err(e) => return format!("gorev yok (satir hatasi: {e})"),
        }
    }

    if lines.is_empty() {
        return "gorev yok".to_string();
    }
    lines.insert(0, format!("omnitrix tasks ({}):", lines.len()));
    lines.join("\n")
}

impl SlashCommand for OmniTasksCommand {
    fn name(&self) -> &str {
        "omni-tasks"
    }

    fn description(&self) -> &str {
        "List tasks recorded by the omnitrix core"
    }

    fn usage(&self) -> &str {
        "/omni-tasks"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Message(list_tasks())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    fn ctx<'a>(models: &'a ModelState, bundle: &'a BundleState) -> CommandExecCtx<'a> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            pager_state: PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..PagerLocalSnapshot::default()
            },
        }
    }

    fn run() -> CommandResult {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        OmniTasksCommand::new().run(&mut c, "")
    }

    /// A temp-dir OMNI_DATA_HOME so the test never touches the real store and
    /// is hermetic. Restored by `Drop`.
    struct ScopedDataDir(std::path::PathBuf);

    impl ScopedDataDir {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir").into_path();
            // Unsafe in edition 2024; single-threaded test scope (serialized
            // against other env-touching tests via `#[serial_test::serial]`).
            unsafe {
                std::env::set_var("XDG_DATA_HOME", &dir);
            }
            Self(dir)
        }
    }

    impl Drop for ScopedDataDir {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            }
        }
    }

    #[test]
    #[serial_test::serial(OMNI_TASKS_DIR)]
    fn no_database_reports_no_tasks() {
        let _scoped = ScopedDataDir::new();
        match run() {
            CommandResult::Message(msg) => assert!(msg.contains("gorev yok")),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    #[serial_test::serial(OMNI_TASKS_DIR)]
    fn empty_tasks_table_reports_no_tasks() {
        let _scoped = ScopedDataDir::new();
        let path = omnitrix_db_path().expect("data dir");
        std::fs::create_dir_all(path.parent().expect("db parent")).expect("mkdir");
        let conn = rusqlite::Connection::open(&path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_id INTEGER,
                root_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                depth INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                closed_at TEXT
            );",
        )
        .expect("create schema");
        drop(conn);

        match run() {
            CommandResult::Message(msg) => assert_eq!(msg, "gorev yok"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    #[serial_test::serial(OMNI_TASKS_DIR)]
    fn lists_tasks_from_store() {
        let _scoped = ScopedDataDir::new();
        let path = omnitrix_db_path().expect("data dir");
        std::fs::create_dir_all(path.parent().expect("db parent")).expect("mkdir");
        let conn = rusqlite::Connection::open(&path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_id INTEGER,
                root_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                depth INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                closed_at TEXT
            );",
        )
        .expect("create schema");
        conn.execute(
            "INSERT INTO tasks (id, parent_id, root_id, title, mode, status, depth) \
             VALUES (1, NULL, 1, ?1, ?2, ?3, 0)",
            ["ilk gorev", "auto", "running"],
        )
        .expect("insert 1");
        conn.execute(
            "INSERT INTO tasks (id, parent_id, root_id, title, mode, status, depth) \
             VALUES (2, NULL, 2, ?1, ?2, ?3, 0)",
            ["ikinci gorev", "manual", "done"],
        )
        .expect("insert 2");
        drop(conn);

        match run() {
            CommandResult::Message(msg) => {
                let lines: Vec<&str> = msg.lines().collect();
                assert_eq!(lines.len(), 3, "header + 2 tasks, got: {msg}");
                assert_eq!(lines[0], "omnitrix tasks (2):");
                assert!(lines[1].contains("#1") && lines[1].contains("ilk gorev"));
                assert!(lines[2].contains("#2") && lines[2].contains("ikinci gorev"));
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn metadata() {
        let cmd = OmniTasksCommand::new();
        assert_eq!(cmd.name(), "omni-tasks");
        assert!(!cmd.description().is_empty());
        assert_eq!(cmd.usage(), "/omni-tasks");
        assert!(!cmd.takes_args());
    }
}
