//! Test yardimcilari (yalniz `cfg(test)`).
//!
//! `event_log` ve `replay` modullerinin testleri gercek semaya ihtiyac duyar:
//! `file_touches.agent_id` -> `agents(id)` ve `agents.task_id` -> `tasks(id)`
//! yabanci anahtarlari var (migrations 0003 + 0008), dolayisiyla bos bir DB
//! uzerinde ekleme basarisiz olur. Buradaki `semali_db` migration'lari kosar
//! ve tek bir gorev/ajan tohumu birakir.

use std::path::{Path, PathBuf};

use omni_proto::{AgentId, FileTouch, StateEvent, now};
use omni_storage::sqlite_schema::SchemaManager;

/// Tohum ajanin kimligi. Testlerdeki `agent_id` degerleri bununla eslesmelidir.
pub const SEED_AGENT_ID: AgentId = 1;

/// Gecici dizinde semasi kurulmus, tek gorev + tek ajan tohumlu bir SQLite acar.
///
/// Donen deger DB dosyasinin yoludur; cagiran onu `EventLog::open`'a verir.
pub fn semali_db(dir: &Path) -> PathBuf {
    let db_path = dir.join("omnitrix.sqlite");

    let schema = SchemaManager::new(&db_path).expect("sema yoneticisi acilmali");
    schema.run_migrations().expect("migration'lar kosmali");

    let conn = rusqlite::Connection::open(&db_path).expect("tohum icin baglanti");
    conn.execute_batch(
        // Testler 1, 2 ve 5 numarali ajanlara olay yaziyor; `file_touches.agent_id`
        // ve `agent_events.agent_id` -> `agents(id)` yabanci anahtari oldugu icin
        // tohumlanmayan her kimlik "FOREIGN KEY constraint failed" verir.
        "INSERT INTO tasks (id, root_id, title, mode, status, depth)
             VALUES (1, 1, 'test gorevi', 'user_focused', 'running', 0);
         INSERT INTO agents (id, task_id, persona, state) VALUES
             (1, 1, 'test-persona', 'init'),
             (2, 1, 'test-persona', 'init'),
             (3, 1, 'test-persona', 'init'),
             (4, 1, 'test-persona', 'init'),
             (5, 1, 'test-persona', 'init');",
    )
    .expect("tohum satirlari yazilmali");

    db_path
}

/// Verilen ajana ait ornek bir dosya dokunusu olayi (5.2 diff akisi).
pub fn ornek_dokunus(agent_id: AgentId) -> StateEvent {
    StateEvent::FileTouched(FileTouch {
        id: None,
        agent_id,
        path: "src/lib.rs".to_string(),
        outside_workspace: false,
        added: 3,
        removed: 1,
        pre_ref: None,
        post_ref: None,
        ts: now(),
    })
}
