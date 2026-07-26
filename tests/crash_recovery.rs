//! Faz 1 dayaniklilik kapisi — MASTER-PLAN 8.1.
//!
//! Kapinin sozu: **rastgele `kill -9` + yeniden baslatma altinda her taahhut
//! edilmis islem YA TAM YA REPLAY edilir; hicbiri yarim kalmaz.**
//!
//! Mekanizma (omni-storage/src/events.rs, I7):
//!   1. Yan etkili satirdan ONCE `write_journal`'a niyet dusulur
//!      (`op_id` UNIQUE, `applied = 0`).
//!   2. Asil INSERT + `applied = 1` guncellemesi tek `WriteOp::Batch` icinde,
//!      yani tek `BEGIN IMMEDIATE`/`COMMIT` icinde gider.
//!   3. Acilista `EventWriter::recover()` yarim kalan niyetleri sahne tablosuna
//!      tasiyip asil tabloya idempotent olarak geri uygular.
//!
//! ## I1 — girdi / beklenen cikti / esik
//!
//! | Kapi | GIRDI | BEKLENEN CIKTI | ESIK |
//! |------|-------|----------------|------|
//! | [`chaos_random_kill9_never_leaves_half_commit`] | `CHAOS_ROUNDS = 4` tur; her turda ayri bir cocuk surec olay yazar ve `[400, 900] ms` arasi rastgele bir gecikmeden sonra `SIGKILL` alir | Her turdan sonra: `write_journal`'da `applied = 0` niyet yok, sahne tablosu bos, her niyet icin `agent_events`'te **tam olarak 1** satir var | Yarim taahhut sayisi **= 0** (tolerans yok); kaosun gercekten is uretmis olmasi icin toplam kurtarilan olay **>= `MIN_EVENTS` (10)** |
//! | [`killed_process_intent_is_replayed_on_restart`] | Tek cocuk surec, `600 ms` sonra `SIGKILL` | Kurtarma sonrasi `applied = 0` niyet yok; kayitli her `seq` `agent_events`'te tekil | Yarim taahhut **= 0**, yazilan olay **>= 1** |
//! | [`staged_intent_is_reapplied_exactly_once`] | Elle yazilmis 1 niyet (`applied = 0`, `seq = 4242`) | `recover()` -> `reapplied = 1`, `agent_events`'te 1 satir | Ikinci `recover()` cagrisindan sonra da satir sayisi **= 1** (idempotans) |
//!
//! Kaos tohumu `OMNI_TESTS_CHAOS_SEED` ile sabitlenebilir; basarisiz kosuda
//! kullanilan tohum hata mesajina basilir.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use omni_storage::events::{AgentEventRecord, EventWriter};
use omni_storage::wal::WalReplay;
use omni_storage::writer_actor::{WriteOp, WriterActor};
use omni_tests::{
    ChaosRng, ENV_CHAOS_SEED, ENV_CHILD_CAS, ENV_CHILD_DB, ENV_CHILD_MAX_MS, KvDb, SEED_AGENT_ID,
    TestDb,
};
use rusqlite::Connection;
use tokio::sync::oneshot;

/// Kaos turu sayisi.
const CHAOS_ROUNDS: usize = 4;
/// Cocugun yasayacagi rastgele araligin alt siniri (ms).
const KILL_DELAY_MIN_MS: u64 = 400;
/// Cocugun yasayacagi rastgele araligin ust siniri (ms).
const KILL_DELAY_MAX_MS: u64 = 900;
/// Cocuk kendiliginden bu sureden fazla yasamaz (oksuz surec emniyeti).
const CHILD_MAX_MS: u64 = 5_000;
/// Kaos kosusunun anlamli sayilmasi icin gereken en az kurtarilmis olay sayisi.
const MIN_EVENTS: i64 = 10;
/// Yarim taahhut toleransi. Bu esik bilerek sifirdir.
const MAX_HALF_COMMITS: usize = 0;

/// Cocuk surec giris noktasinin adi; [`crash_worker_entrypoint`] ile ayni olmali.
const WORKER_TEST: &str = "crash_worker_entrypoint";

// ---------------------------------------------------------------------------
// Bolum 1 — WalReplay / WriterActor birim kapilari
// ---------------------------------------------------------------------------

#[test]
fn wal_replay_on_startup() {
    let db = KvDb::new("testns");

    {
        let conn = db.connect();
        omni_tests::insert_journal_entry(&conn, "op-001", "testns", "k1", b"v1", "SET", false);
        omni_tests::insert_journal_entry(&conn, "op-002", "testns", "k2", b"v2", "SET", false);
        omni_tests::insert_journal_entry(&conn, "op-003", "testns", "k3", b"v3", "SET", false);
    }

    let replay = WalReplay::new(db.db_path()).expect("WalReplay::new");
    let report = replay.replay_pending().expect("replay");

    // GIRDI: 3 bekleyen niyet. BEKLENEN: hepsi oynatilir, hicbiri basarisiz olmaz.
    assert_eq!(report.total_ops, 3);
    assert_eq!(report.replayed, 3);
    assert_eq!(report.failed, 0);

    let conn = db.connect();
    assert_eq!(omni_tests::unapplied_intents(&conn), 0);
    assert_eq!(omni_tests::applied_intents(&conn), 3);
    assert_eq!(
        omni_tests::scalar_count(&conn, "SELECT COUNT(*) FROM kv_testns"),
        3
    );
}

#[test]
fn idempotent_replay() {
    let db = KvDb::new("testns");

    {
        let conn = db.connect();
        omni_tests::insert_journal_entry(&conn, "op-a", "testns", "key1", b"val1", "SET", false);
        omni_tests::insert_journal_entry(&conn, "op-b", "testns", "key2", b"val2", "SET", false);
    }

    let replay = WalReplay::new(db.db_path()).expect("WalReplay::new");

    let first = replay.replay_pending().expect("ilk replay");
    assert_eq!(first.total_ops, 2);
    assert_eq!(first.replayed, 2);
    assert_eq!(first.failed, 0);

    // ESIK: ikinci replay hicbir sey yapmamali (idempotans).
    let second = replay.replay_pending().expect("ikinci replay");
    assert_eq!(second.total_ops, 0, "ikinci replay: hepsi zaten uygulanmis");
    assert_eq!(second.replayed, 0);
    assert_eq!(second.failed, 0);

    let conn = db.connect();
    assert_eq!(omni_tests::unapplied_intents(&conn), 0);
    assert_eq!(omni_tests::applied_intents(&conn), 2);
    assert_eq!(
        omni_tests::scalar_count(&conn, "SELECT COUNT(*) FROM kv_testns"),
        2
    );
}

#[tokio::test]
async fn writer_actor_writes_survive_replay() {
    let db = KvDb::new("testns");

    // Kaza oncesi durum: 3 niyet yazili, hicbiri uygulanmamis.
    {
        let conn = db.connect();
        omni_tests::insert_journal_entry(&conn, "crash-001", "testns", "alpha", b"aaa", "SET", false);
        omni_tests::insert_journal_entry(&conn, "crash-002", "testns", "beta", b"bbb", "SET", false);
        omni_tests::insert_journal_entry(&conn, "crash-003", "testns", "gamma", b"ggg", "SET", false);
    }

    // Iki tanesi kazadan once gercekten yazilmis olsun.
    let mut actor = WriterActor::new(db.db_path());
    for (key, value) in [("alpha", b"aaa"), ("beta", b"bbb")] {
        let (reply, rx) = oneshot::channel();
        actor
            .write(WriteOp::Set {
                namespace: "testns".into(),
                key: key.into(),
                value: value.to_vec(),
                reply,
            })
            .await
            .expect("gonderim");
        rx.await.expect("yanit").expect("yazim");
    }
    actor.shutdown().await;

    let report = WalReplay::new(db.db_path())
        .expect("WalReplay::new")
        .replay_pending()
        .expect("kurtarma");

    assert_eq!(report.total_ops, 3);
    assert_eq!(report.replayed, 3);
    assert_eq!(report.failed, 0);

    let conn = db.connect();
    assert_eq!(omni_tests::unapplied_intents(&conn), 0);

    // BEKLENEN CIKTI: zaten yazilmis anahtarlar tekrarlanmaz, eksik olan tamamlanir.
    let mut stmt = conn
        .prepare("SELECT value FROM kv_testns ORDER BY key")
        .expect("sorgu");
    let values: Vec<String> = stmt
        .query_map([], |r| {
            let v: Vec<u8> = r.get(0)?;
            Ok(String::from_utf8_lossy(&v).into_owned())
        })
        .expect("query_map")
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(values, vec!["aaa", "bbb", "ggg"]);
}

// ---------------------------------------------------------------------------
// Bolum 2 — EventWriter kurtarmasi (deterministik)
// ---------------------------------------------------------------------------

/// GIRDI: elle yazilmis tek bir niyet (`applied = 0`, `seq = 4242`).
/// BEKLENEN CIKTI: `recover()` niyeti `agent_events`'e tasir.
/// ESIK: ikinci `recover()` sonrasi satir sayisi hala 1 — replay idempotandir.
#[tokio::test]
async fn staged_intent_is_reapplied_exactly_once() {
    const STAGED_SEQ: i64 = 4242;

    let db = TestDb::new();
    let writer = EventWriter::open(db.db_path(), db.cas_path()).expect("EventWriter::open");

    // `EventWriter` acildiktan sonra `write_journal` semasi hizalanmis olur;
    // niyeti ancak bu noktada elle yazabiliriz.
    {
        let conn = db.connect();
        let payload = staged_intent_json("op-staged", STAGED_SEQ);
        omni_tests::insert_journal_entry(
            &conn,
            "op-staged",
            "omni_events",
            "op-staged",
            payload.as_bytes(),
            "SET",
            false,
        );
    }

    let report = writer.recover().expect("recover");
    assert_eq!(report.replayed, 1, "niyet sahne tablosuna tasinmali");
    assert_eq!(report.reapplied, 1, "niyet asil tabloya uygulanmali");
    assert_eq!(report.failed, 0);

    let conn = db.connect();
    assert_eq!(
        omni_tests::scalar_count(
            &conn,
            &format!("SELECT COUNT(*) FROM agent_events WHERE seq = {STAGED_SEQ}")
        ),
        1,
        "niyet tam olarak bir kez uygulanmali"
    );
    assert_eq!(omni_tests::unapplied_intents(&conn), 0);
    assert_eq!(staging_rows(&conn), 0, "sahne tablosu bosaltilmali");

    // ESIK: tekrar kurtarma hicbir sey degistirmemeli.
    let again = writer.recover().expect("ikinci recover");
    assert_eq!(again.reapplied, 0);
    assert_eq!(again.failed, 0);
    assert_eq!(
        omni_tests::scalar_count(
            &conn,
            &format!("SELECT COUNT(*) FROM agent_events WHERE seq = {STAGED_SEQ}")
        ),
        1,
        "ikinci kurtarma satiri cogaltmamali"
    );
}

// ---------------------------------------------------------------------------
// Bolum 3 — gercek surec olumu
// ---------------------------------------------------------------------------

/// GIRDI: bir cocuk surec olay yazarken 600 ms sonra `SIGKILL` alir.
/// BEKLENEN CIKTI: yeniden acilista `recover()` yarim niyeti tamamlar.
/// ESIK: yarim taahhut = 0, kurtarma sonrasi olay sayisi >= 1.
#[tokio::test]
async fn killed_process_intent_is_replayed_on_restart() {
    let db = TestDb::new();

    let stderr = run_child_and_kill(&db, Duration::from_millis(600));

    let recovered = recover_and_check(&db, &stderr);

    assert!(
        recovered >= 1,
        "cocuk hicbir olay taahhut edememis; kaos anlamsiz.\ncocuk stderr:\n{stderr}"
    );
}

/// Faz 1 kapisi: rastgele `kill -9` + restart dongusu.
///
/// GIRDI: `CHAOS_ROUNDS` tur; her tur ayri bir cocuk surec, `[KILL_DELAY_MIN_MS,
/// KILL_DELAY_MAX_MS]` arasi rastgele bir gecikmeden sonra `SIGKILL`.
/// BEKLENEN CIKTI: her turdan sonra tutarli depo — yarim taahhut yok, cift
/// satir yok, bekleyen niyet yok.
/// ESIK: yarim taahhut `<= MAX_HALF_COMMITS (0)`; toplam olay `>= MIN_EVENTS`.
#[tokio::test]
async fn chaos_random_kill9_never_leaves_half_commit() {
    let db = TestDb::new();
    let (mut rng, seed) = ChaosRng::from_env_or_clock();

    eprintln!("kaos tohumu: {seed} ({ENV_CHAOS_SEED} ile sabitlenebilir)");

    let mut last_stderr = String::new();
    let mut previous = 0i64;
    for round in 0..CHAOS_ROUNDS {
        let delay = rng.range(KILL_DELAY_MIN_MS, KILL_DELAY_MAX_MS);
        last_stderr = run_child_and_kill(&db, Duration::from_millis(delay));

        let recovered = recover_and_check(&db, &last_stderr);
        // Kurtarma hicbir satiri kaybetmemeli; sayim yalniz artabilir.
        assert!(
            recovered >= previous,
            "tur {round}: olay sayisi {previous} -> {recovered} olarak geriledi (tohum {seed})"
        );
        eprintln!("tur {round}: {delay} ms sonra SIGKILL, toplam olay {recovered}");
        previous = recovered;
    }

    assert!(
        previous >= MIN_EVENTS,
        "kaos kosusu {CHAOS_ROUNDS} turda yalniz {previous} olay uretti, esik {MIN_EVENTS}. \
         Tohum {seed}. Son cocuk stderr:\n{last_stderr}"
    );
}

/// Cocuk surec giris noktasi. Dogrudan kosulmaz; [`omni_tests::spawn_self_test`]
/// bu ikiliyi `--ignored --exact` ile yeniden calistirir.
///
/// Yaptigi is: depoyu acar, acilis kurtarmasini kosar, sonra oldurulene kadar
/// kesintisiz olay yazar. Kapanis kancasi yoktur — `SIGKILL` geldiginde hicbir
/// temizlik calismaz, tam da kapinin test etmek istedigi durum budur.
#[test]
#[ignore = "cocuk surec giris noktasi; ebeveyn test kosar"]
fn crash_worker_entrypoint() {
    let Ok(db_path) = std::env::var(ENV_CHILD_DB) else {
        // Env yoksa bu ikili elle `--ignored` ile kosulmus demektir: hicbir sey yapma.
        return;
    };
    let cas_path = std::env::var(ENV_CHILD_CAS).unwrap_or_default();
    let max_ms: u64 = std::env::var(ENV_CHILD_MAX_MS)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CHILD_MAX_MS);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async move {
        let writer =
            EventWriter::open(Path::new(&db_path), Path::new(&cas_path)).expect("EventWriter::open");

        // Acilis kurtarmasi: onceki turdan kalan yarim niyetler once tamamlanir.
        // `seq` tahsisi `agent_events`'teki MAX'tan devam ettigi icin bu sira sart.
        let report = writer.recover().expect("acilis kurtarmasi");
        eprintln!(
            "cocuk: kurtarma replayed={} reapplied={} failed={}",
            report.replayed, report.reapplied, report.failed
        );

        let deadline = Instant::now() + Duration::from_millis(max_ms);
        let mut n: u64 = 0;
        while Instant::now() < deadline {
            writer
                .record_event(AgentEventRecord {
                    agent_id: SEED_AGENT_ID,
                    kind: "chaos_tick".into(),
                    payload_json: Some(format!("{{\"n\":{n}}}")),
                })
                .await
                .expect("record_event");
            n += 1;
        }
        eprintln!("cocuk: {n} olay yazildi (oldurulmedi)");
    });
}

// ---------------------------------------------------------------------------
// Yardimcilar
// ---------------------------------------------------------------------------

/// Cocugu baslatir, `delay` kadar bekler, `SIGKILL` gonderir ve stderr'ini doner.
fn run_child_and_kill(db: &TestDb, delay: Duration) -> String {
    let child = omni_tests::spawn_self_test(
        WORKER_TEST,
        &[
            (ENV_CHILD_DB, db.db_path().display().to_string()),
            (ENV_CHILD_CAS, db.cas_path().display().to_string()),
            (ENV_CHILD_MAX_MS, CHILD_MAX_MS.to_string()),
        ],
    );
    std::thread::sleep(delay);
    omni_tests::kill9_and_collect(child)
}

/// Yeniden acilis + kurtarma; ardindan 8.1 degismezlerini dogrular.
/// Doner deger: kurtarma sonrasi `agent_events` satir sayisi.
fn recover_and_check(db: &TestDb, child_stderr: &str) -> i64 {
    // Olum aninda kac niyet yarim kalmis? Bu sayi kaosun replay yolunu gercekten
    // tetikledigini gosterir; sifir olabilecegi icin assert edilmez, raporlanir.
    let pending_before = omni_tests::unapplied_intents(&db.connect());

    let writer = EventWriter::open(db.db_path(), db.cas_path()).expect("yeniden acilis");
    let report = writer.recover().expect("recover");
    eprintln!(
        "  kurtarma: olum aninda bekleyen={pending_before} replayed={} reapplied={} failed={}",
        report.replayed, report.reapplied, report.failed
    );
    assert_eq!(
        report.failed, 0,
        "kurtarma sirasinda cozulemeyen niyet var.\ncocuk stderr:\n{child_stderr}"
    );

    let conn = db.connect();

    // 1) Bekleyen niyet kalmamali.
    assert_eq!(
        omni_tests::unapplied_intents(&conn),
        0,
        "kurtarma sonrasi applied=0 niyet kaldi.\ncocuk stderr:\n{child_stderr}"
    );

    // 2) Sahne tablosu bosalmis olmali.
    assert_eq!(
        staging_rows(&conn),
        0,
        "sahne tablosunda kalinti var.\ncocuk stderr:\n{child_stderr}"
    );

    // 3) Her taahhut edilmis niyet icin asil tabloda TAM OLARAK bir satir.
    // Esik sifir oldugu icin karsilastirma esitliktir; `<=` clippy'nin
    // `absurd_extreme_comparisons` kuralina takilir.
    let half = half_commits(&conn);
    assert_eq!(
        half.len(),
        MAX_HALF_COMMITS,
        "yarim taahhut bulundu (esik {MAX_HALF_COMMITS}): {half:?}\ncocuk stderr:\n{child_stderr}"
    );

    // 4) `agent_events` icinde cift `seq` olmamali.
    let rows = omni_tests::scalar_count(&conn, "SELECT COUNT(*) FROM agent_events");
    let distinct = omni_tests::scalar_count(&conn, "SELECT COUNT(DISTINCT seq) FROM agent_events");
    assert_eq!(
        rows, distinct,
        "agent_events'te cift seq var ({rows} satir / {distinct} tekil).\ncocuk stderr:\n{child_stderr}"
    );

    rows
}

/// `write_journal`'daki her niyeti asil tabloyla karsilastirir; asil tabloda
/// tam olarak bir satiri olmayan niyetleri `(op_id, seq, bulunan_satir)` olarak
/// dondurur. Bos liste = yarim taahhut yok.
fn half_commits(conn: &Connection) -> Vec<(String, i64, i64)> {
    let mut broken = Vec::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();

    for (op_id, payload) in omni_tests::journal_payloads(conn) {
        let Some(seq) = intent_seq(&payload) else {
            continue;
        };
        // Ayni `seq` iki farkli niyette gorunmemeli; gorunurse tahsis bozulmus.
        if !seen.insert(seq) {
            broken.push((op_id, seq, -1));
            continue;
        }
        let found = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_events WHERE agent_id = ?1 AND seq = ?2",
                rusqlite::params![SEED_AGENT_ID, seq],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0);
        if found != 1 {
            broken.push((op_id, seq, found));
        }
    }

    broken
}

/// Niyet govdesinden `AgentEvent.seq` degerini cikarir. Govde `EventWriter`'in
/// yazdigi JSON'dur: `{"op_id":..,"ts":..,"write":{"AgentEvent":{"seq":N,..}}}`.
fn intent_seq(payload: &[u8]) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    value
        .get("write")?
        .get("AgentEvent")?
        .get("seq")?
        .as_i64()
}

/// Sahne tablosundaki (`kv_omni_events`) bekleyen satir sayisi.
fn staging_rows(conn: &Connection) -> i64 {
    omni_tests::scalar_count(conn, "SELECT COUNT(*) FROM kv_omni_events")
}

/// `EventWriter`'in `write_journal.value` icin bekledigi niyet govdesi.
fn staged_intent_json(op_id: &str, seq: i64) -> String {
    serde_json::json!({
        "op_id": op_id,
        "ts": "2026-01-01T00:00:00+00:00",
        "write": {
            "AgentEvent": {
                "agent_id": SEED_AGENT_ID,
                "seq": seq,
                "kind": "staged_intent",
                "payload_json": serde_json::Value::Null,
            }
        }
    })
    .to_string()
}
