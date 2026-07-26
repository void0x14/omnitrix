//! Kok `tests/` altindaki entegrasyon kapilarinin paylastigi yardimcilar.
//!
//! MASTER-PLAN Bolum 4 klasor yapisi entegrasyon testlerini deponun kokundeki
//! `tests/` dizininde gosterir. Kok `Cargo.toml` sanal bir workspace oldugu icin
//! (`[package]` yok) o dosyalar hicbir crate tarafindan derlenmiyordu. Bu crate
//! her kok test dosyasi icin bir `[[test]]` hedefi tanimlar; dosyalar planin
//! dedigi yerde kalir, `cargo test -p omni-tests` ile derlenir ve kosar.
//!
//! Buradaki yardimcilar uc isi toplar:
//!   * gecici DB kurulumu + migration kosumu ([`TestDb`]),
//!   * `write_journal` niyet kaydinin okunmasi/yazilmasi (8.1 / I7),
//!   * kaos testleri icin surec baslatma + `SIGKILL` ve tekrarlanabilir RNG.
//!
//! Bu crate yalniz test kosumunda kullanilir; `expect` kullanimi serbesttir.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use omni_storage::events::EventWriter;
use omni_storage::sqlite_schema::SchemaManager;
use rusqlite::Connection;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Cocuk surec sozlesmesi
// ---------------------------------------------------------------------------

/// Cocuk surece hedef SQLite dosyasini tasiyan cevre degiskeni.
pub const ENV_CHILD_DB: &str = "OMNI_TESTS_CHILD_DB";

/// Cocuk surece CAS kok dizinini tasiyan cevre degiskeni.
pub const ENV_CHILD_CAS: &str = "OMNI_TESTS_CHILD_CAS";

/// Cocugun kendiliginden duracagi ust sinir (ms). Ebeveyn normalde bundan once
/// `SIGKILL` gonderir; bu deger yalniz oksuz surec birakmamak icin bir emniyet
/// kemeridir.
pub const ENV_CHILD_MAX_MS: &str = "OMNI_TESTS_CHILD_MAX_MS";

/// Kaos turlarinin tohumu. Verilmezse saatten turetilir ve teste basilir, boylece
/// basarisiz bir kosu birebir tekrar edilebilir.
pub const ENV_CHAOS_SEED: &str = "OMNI_TESTS_CHAOS_SEED";

/// [`TestDb`] tarafindan acilan tohum ajanin `agents.id` degeri.
pub const SEED_AGENT_ID: i64 = 1;

// ---------------------------------------------------------------------------
// Gecici DB
// ---------------------------------------------------------------------------

/// Migration'lari kosulmus, tek ajanla tohumlanmis gecici bir SQLite deposu.
///
/// `TempDir` yapinin icinde tutulur; deger dusunce dizin de silinir. (Eski
/// `TempDir::into_path` kullanimi hem dizini sizdiriyor hem de kullanimdan
/// kaldirilmis API uyarisi uretiyordu.)
pub struct TestDb {
    _dir: TempDir,
    db_path: PathBuf,
    cas_path: PathBuf,
}

impl TestDb {
    /// Bos bir depo acar: migration'lar kosar, `tasks`/`agents` tohumlanir.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("tempdir olusturulamadi");
        // `xai-sqlite-journal` ag dosya sistemlerinde dosya adini degistirebilir;
        // migration'lar ve EventWriter ayni gercek yolu gormeli.
        let db_path = EventWriter::effective_db_path(&dir.path().join("omni.db"));
        let cas_path = dir.path().join("cas");

        run_migrations(&db_path);
        {
            let conn = open(&db_path);
            seed_task_and_agent(&conn);
        }

        Self {
            _dir: dir,
            db_path,
            cas_path,
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn cas_path(&self) -> &Path {
        &self.cas_path
    }

    /// Yeni bir okuma/yazma baglantisi acar.
    pub fn connect(&self) -> Connection {
        open(&self.db_path)
    }
}

impl Default for TestDb {
    fn default() -> Self {
        Self::new()
    }
}

/// Migration kosumu olmadan, yalniz `write_journal` + verilen kv tablosu iceren
/// cok ince bir depo. `WalReplay`/`WriterActor` birim kapilari icin yeterlidir.
pub struct KvDb {
    _dir: TempDir,
    db_path: PathBuf,
}

impl KvDb {
    /// `kv_{namespace}` tablosunu ve `write_journal`'i kurar.
    pub fn new(namespace: &str) -> Self {
        let dir = TempDir::new().expect("tempdir olusturulamadi");
        let db_path = dir.path().join("kv.db");
        {
            let conn = open(&db_path);
            omni_storage::wal::WalReplay::ensure_write_journal(&conn)
                .expect("write_journal kurulamadi");
            ensure_kv_table(&conn, namespace);
        }
        Self {
            _dir: dir,
            db_path,
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn connect(&self) -> Connection {
        open(&self.db_path)
    }
}

/// SQLite baglantisi acar (WAL + foreign_keys ON).
pub fn open(db_path: &Path) -> Connection {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("db dizini olusturulamadi");
    }
    let conn = Connection::open(db_path).expect("SQLite acilamadi");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("journal_mode=WAL");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("foreign_keys=ON");
    conn
}

/// `migrations/` altindaki tum surumleri uygular.
pub fn run_migrations(db_path: &Path) {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("db dizini olusturulamadi");
    }
    SchemaManager::new(db_path)
        .expect("SchemaManager acilamadi")
        .run_migrations()
        .expect("migration kosumu basarisiz");
}

/// Bir kok gorev + ona bagli bir ajan acar; `agent_id` doner.
///
/// `agent_events` / `messages` / `tool_calls` / `file_touches` tablolarinin
/// `agent_id` sutunu `agents(id)`'ye referans verdigi icin, `foreign_keys=ON`
/// olan bir baglantida bu tohum sart.
pub fn seed_task_and_agent(conn: &Connection) -> i64 {
    conn.execute(
        "INSERT OR IGNORE INTO tasks (id, parent_id, root_id, title, mode, status, depth)
         VALUES (1, NULL, 1, 'faz1-kapisi', 'build', 'running', 0)",
        [],
    )
    .expect("tasks tohumlanamadi");
    conn.execute(
        "INSERT OR IGNORE INTO agents (id, task_id, persona, parent_agent_id, state)
         VALUES (?1, 1, 'explorer', NULL, 'active')",
        rusqlite::params![SEED_AGENT_ID],
    )
    .expect("agents tohumlanamadi");
    SEED_AGENT_ID
}

/// `kv_{namespace}` replay hedefini kurar.
pub fn ensure_kv_table(conn: &Connection, namespace: &str) {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS kv_{namespace} (key TEXT PRIMARY KEY, value BLOB)"
    ))
    .expect("kv tablosu kurulamadi");
}

// ---------------------------------------------------------------------------
// write_journal yardimcilari
// ---------------------------------------------------------------------------

/// `write_journal`'a ham bir niyet satiri dusurur (`applied` cagirandan gelir).
pub fn insert_journal_entry(
    conn: &Connection,
    op_id: &str,
    namespace: &str,
    key: &str,
    value: &[u8],
    op_type: &str,
    applied: bool,
) {
    let has_op_kind = table_has_column(conn, "write_journal", "op_kind");
    let sql = if has_op_kind {
        "INSERT INTO write_journal (op_id, namespace, key, value, op_type, op_kind, applied)
         VALUES (?1, ?2, ?3, ?4, ?5, 'omni_event', ?6)"
    } else {
        "INSERT INTO write_journal (op_id, namespace, key, value, op_type, applied)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
    };
    conn.execute(
        sql,
        rusqlite::params![op_id, namespace, key, value, op_type, i64::from(applied)],
    )
    .expect("niyet kaydi yazilamadi");
}

/// Tek satirlik sayim sorgusu; sonuc yoksa 0.
pub fn scalar_count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
}

/// Henuz uygulanmamis (`applied = 0`) niyet sayisi.
pub fn unapplied_intents(conn: &Connection) -> i64 {
    scalar_count(conn, "SELECT COUNT(*) FROM write_journal WHERE applied = 0")
}

/// Uygulanmis (`applied = 1`) niyet sayisi.
pub fn applied_intents(conn: &Connection) -> i64 {
    scalar_count(conn, "SELECT COUNT(*) FROM write_journal WHERE applied = 1")
}

/// `write_journal`'daki tum niyet govdelerini `(op_id, value)` olarak dondurur.
/// Govdesi bos olan satirlar atlanir.
pub fn journal_payloads(conn: &Connection) -> Vec<(String, Vec<u8>)> {
    let mut stmt = conn
        .prepare("SELECT op_id, value FROM write_journal ORDER BY id ASC")
        .expect("write_journal sorgusu hazirlanamadi");
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
        })
        .expect("write_journal okunamadi");
    rows.filter_map(|r| r.ok())
        .filter_map(|(op_id, value)| value.map(|v| (op_id, v)))
        .collect()
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = match conn.prepare("SELECT name FROM pragma_table_info(?1)") {
        Ok(s) => s,
        Err(_) => return false,
    };
    let Ok(rows) = stmt.query_map([table], |r| r.get::<_, String>(0)) else {
        return false;
    };
    rows.filter_map(|r| r.ok()).any(|name| name == column)
}

// ---------------------------------------------------------------------------
// Surec baslatma + SIGKILL
// ---------------------------------------------------------------------------

/// Ayni test ikilisini yeniden calistirarak bir cocuk surec baslatir.
///
/// `test_name` bu ikilideki `#[ignore]` isaretli bir giris noktasi olmalidir;
/// `--ignored --exact` ikilisi yalniz onu kosar. Boylece "gercek bir surec
/// `SIGKILL` yiyor" senaryosu ayri bir ikili gerektirmeden kurulur.
pub fn spawn_self_test(test_name: &str, envs: &[(&str, String)]) -> Child {
    let exe = std::env::current_exe().expect("current_exe okunamadi");
    let mut cmd = Command::new(exe);
    cmd.args([
        "--exact",
        test_name,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    cmd.spawn().expect("cocuk surec baslatilamadi")
}

/// Cocugu `SIGKILL` ile oldurur (Unix'te `Child::kill` = sinyal 9) ve stderr
/// ciktisini toplar. Kapanis kancasi calismaz: temizlik, flush, Drop yoktur.
pub fn kill9_and_collect(mut child: Child) -> String {
    let _ = child.kill();
    match child.wait_with_output() {
        Ok(out) => String::from_utf8_lossy(&out.stderr).into_owned(),
        Err(e) => format!("<cocuk cikti okunamadi: {e}>"),
    }
}

// ---------------------------------------------------------------------------
// Tekrarlanabilir RNG
// ---------------------------------------------------------------------------

/// xorshift64* — kaos turlarinin gecikmelerini uretir. Tohum teste basildigi
/// icin basarisiz bir kosu birebir tekrar edilebilir.
pub struct ChaosRng {
    state: u64,
}

impl ChaosRng {
    pub fn seeded(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed },
        }
    }

    /// `OMNI_TESTS_CHAOS_SEED` verilmisse onu, yoksa saatten turetilmis bir
    /// tohumu kullanir. Kullanilan tohum ikinci deger olarak doner.
    pub fn from_env_or_clock() -> (Self, u64) {
        let seed = std::env::var(ENV_CHAOS_SEED)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0x5DEE_CE66_D1CE_B00Bu64)
            });
        (Self::seeded(seed), seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `[lo, hi]` kapali araliginda bir deger.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_u64() % (hi - lo + 1)
    }
}
